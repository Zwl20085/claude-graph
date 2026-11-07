//! Live scheduler turns (one per graph folder) and the `AppState` methods
//! behind the assistant commands. Used by the Tauri commands and directly by
//! the end-to-end tests.
//!
//! Every state change happens under the registry lock, so a turn cannot start
//! while another runs for the same folder, and the stale-`working` sweep never
//! touches a session whose turn is alive. A turn's registry entry is removed
//! only after its outcome is recorded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::Utc;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::contract::{
    AppError, AppResult, AssistantKind, AssistantRole, AssistantSession, AssistantState,
    AssistantStatus,
};
use crate::events::Emitter;
use crate::paths::abs;
use crate::run_api::AppState;
use crate::run_loop::blocking;
use crate::run_store::INTERRUPTED;
use crate::runner::{run_turn, TurnSpec};

use super::journal::{append_messages, new_message, read_session, read_state, write_session};
use super::turn::{prepare, record};

/// App message when the user resets the session.
pub const RESET_MESSAGE: &str =
    "Session reset: the next instruction starts a fresh scheduler session";

struct LiveTurn {
    kill: watch::Sender<bool>,
    /// Taken by [`AppState::assistant_wait`].
    join: Option<JoinHandle<()>>,
}

type TurnMap = HashMap<PathBuf, LiveTurn>;

/// Registry of running scheduler turns, keyed by canonical graph folder.
#[derive(Clone, Default)]
pub struct AssistantTurns {
    inner: Arc<Mutex<TurnMap>>,
}

impl AssistantTurns {
    fn lock(&self) -> MutexGuard<'_, TurnMap> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn live_count(&self) -> usize {
        self.lock().len()
    }

    /// Ask every running turn to kill its process tree; returns how many.
    pub fn kill_all(&self) -> usize {
        let map = self.lock();
        map.values().for_each(|t| {
            t.kill.send_replace(true);
        });
        map.len()
    }
}

/// Removes the registry entry when the turn task ends, also by panic.
struct Deregister {
    turns: AssistantTurns,
    dir: PathBuf,
}

impl Drop for Deregister {
    fn drop(&mut self) {
        self.turns.lock().remove(&self.dir);
    }
}

fn busy() -> AppError {
    AppError::new(
        "assistant_busy",
        "the scheduler is still working on the last message; wait for it or cancel it",
    )
}

/// A session left `working` with no live turn (the app exited or crashed)
/// becomes `failed` / "interrupted" with an app error message. Caller holds the lock.
fn sweep_stale(map: &TurnMap, dir: &Path) -> AppResult<()> {
    if map.contains_key(dir) {
        return Ok(());
    }
    let Some(session) = read_session(dir)? else {
        return Ok(());
    };
    if session.status != AssistantStatus::Working {
        return Ok(());
    }
    log::warn!(
        "assistant turn {} of {} was left working; marking it interrupted",
        session.turns,
        dir.display()
    );
    let now = Utc::now();
    let text = format!(
        "Turn {} was interrupted (the app exited or crashed)",
        session.turns
    );
    let message = new_message(
        session.turns,
        AssistantRole::App,
        AssistantKind::Error,
        None,
        &text,
        now,
    );
    append_messages(dir, &[message])?;
    write_session(
        dir,
        &AssistantSession {
            status: AssistantStatus::Failed,
            last_error: Some(INTERRUPTED.to_string()),
            updated_at: now,
            ..session
        },
    )
}

/// Run the turn, record it, leave the registry, then announce the new state.
async fn run_and_record(
    spec: TurnSpec,
    turn: u32,
    resumed: bool,
    emitter: Arc<dyn Emitter>,
    kill: watch::Receiver<bool>,
    guard: Deregister,
) {
    let dir = guard.dir.clone();
    let outcome = run_turn(spec, emitter.clone(), kill).await;
    let recorded = blocking(move || record(&dir, turn, resumed, &outcome, Utc::now())).await;
    let shown = guard.dir.display().to_string();
    drop(guard);
    match recorded {
        Ok(state) => emitter.assistant_update(&state),
        Err(e) => log::error!("recording assistant turn {turn} of {shown} failed: {e}"),
    }
}

impl AppState {
    pub fn assistant_active_count(&self) -> usize {
        self.assistant_turns().live_count()
    }

    /// The assistant state of the graph in `dir` (empty idle state without
    /// `assistant/`), after sweeping a stale `working` session.
    pub fn assistant_state(&self, dir: &Path) -> AppResult<AssistantState> {
        let graph_dir = abs(dir);
        let map = self.assistant_turns().lock();
        sweep_stale(&map, &graph_dir)?;
        read_state(&graph_dir)
    }

    /// Append the user's message and start a scheduler turn; returns at once.
    /// `dir` should already be canonical ([`crate::paths::existing_dir`]).
    pub fn assistant_send(
        &self,
        dir: &Path,
        text: &str,
        emitter: Arc<dyn Emitter>,
    ) -> AppResult<AssistantState> {
        let text = text.trim();
        if text.is_empty() {
            return Err(AppError::new("invalid_input", "message is empty"));
        }
        let graph_dir = abs(dir);
        let turns = self.assistant_turns().clone();
        let mut map = turns.lock();
        if map.contains_key(&graph_dir) {
            return Err(busy());
        }
        sweep_stale(&map, &graph_dir)?;
        let now = Utc::now();
        let previous = read_session(&graph_dir)?.unwrap_or_else(|| AssistantSession::empty(now));
        if previous.status == AssistantStatus::Working {
            return Err(busy());
        }
        let claude = self.find_claude()?;
        let prepared = prepare(&graph_dir, &previous, text, claude, now)?;
        append_messages(&graph_dir, std::slice::from_ref(&prepared.message))?;
        write_session(&graph_dir, &prepared.session)?;
        let state = read_state(&graph_dir)?;
        let (kill_tx, kill_rx) = watch::channel(false);
        let guard = Deregister {
            turns: turns.clone(),
            dir: graph_dir.clone(),
        };
        // The task cannot leave the registry before the insert below: its
        // guard needs the lock this function holds.
        let join = tokio::spawn(run_and_record(
            prepared.spec,
            prepared.turn,
            prepared.resumed,
            emitter.clone(),
            kill_rx,
            guard,
        ));
        map.insert(
            graph_dir,
            LiveTurn {
                kill: kill_tx,
                join: Some(join),
            },
        );
        drop(map);
        emitter.assistant_update(&state);
        Ok(state)
    }

    /// Kill the running turn of `dir`; the turn task records it as cancelled.
    pub fn assistant_cancel(&self, dir: &Path) -> AppResult<()> {
        let graph_dir = abs(dir);
        let map = self.assistant_turns().lock();
        let live = map.get(&graph_dir).ok_or_else(|| {
            AppError::new("not_found", "no scheduler turn is running for this graph")
        })?;
        live.kill.send_replace(true);
        Ok(())
    }

    /// Forget the CLI session (the next instruction starts fresh). Journal and
    /// turn folders are kept.
    pub fn assistant_reset(
        &self,
        dir: &Path,
        emitter: Arc<dyn Emitter>,
    ) -> AppResult<AssistantState> {
        let graph_dir = abs(dir);
        let map = self.assistant_turns().lock();
        if map.contains_key(&graph_dir) {
            return Err(busy());
        }
        sweep_stale(&map, &graph_dir)?;
        let now = Utc::now();
        let before = read_session(&graph_dir)?.unwrap_or_else(|| AssistantSession::empty(now));
        let message = new_message(
            0,
            AssistantRole::App,
            AssistantKind::System,
            None,
            RESET_MESSAGE,
            now,
        );
        append_messages(&graph_dir, &[message])?;
        write_session(
            &graph_dir,
            &AssistantSession {
                session_id: None,
                workflow_hash: None,
                status: AssistantStatus::Idle,
                last_error: None,
                updated_at: now,
                ..before
            },
        )?;
        let state = read_state(&graph_dir)?;
        drop(map);
        emitter.assistant_update(&state);
        Ok(state)
    }

    /// Wait until no turn runs for `dir` (tests; the commands never block on a turn).
    pub async fn assistant_wait(&self, dir: &Path) {
        let graph_dir = abs(dir);
        loop {
            let join = match self.assistant_turns().lock().get_mut(&graph_dir) {
                None => return,
                Some(live) => live.join.take(),
            };
            match join {
                Some(join) => {
                    if let Err(e) = join.await {
                        log::error!("assistant turn task failed: {e}");
                    }
                }
                None => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }
}

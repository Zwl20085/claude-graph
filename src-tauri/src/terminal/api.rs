//! Live terminal sessions and the functions behind the terminal commands
//! (`list_sessions`, `pty_write`, `pty_resize`, `pty_replay`,
//! `open_session_window`). Sessions stay registered after their run ended
//! (the process may still be alive) until the app exits.
//!
//! Every id and size coming from the frontend is checked here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::contract::{
    AppError, AppResult, PromptAnswer, PtyReplay, SessionPrompt, TerminalSession, TurnPrompt,
};
use crate::events::{Emitter, WindowSpec};
use crate::prompt::display_dir;
use crate::run_store::{check_id, read_run_state};
use crate::terminal::deliver::{Expect, FALLBACK_QUIET};
use crate::terminal::port::spawn_followup;
use crate::terminal::pty::Pty;
use crate::terminal::view::View;
use crate::terminal::window_label;

/// Largest `pty_write` text.
pub const WRITE_MAX: usize = 64 * 1024;
pub const COLS_RANGE: (u16, u16) = (10, 1000);
pub const ROWS_RANGE: (u16, u16) = (4, 500);
const WINDOW_SIZE: (f64, f64) = (1000.0, 640.0);
const WINDOW_MIN: (f64, f64) = (600.0, 400.0);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// One session, shared by its task, the run loop and the commands.
pub struct SessionShared {
    pub run_id: String,
    pub graph_dir: PathBuf,
    info: Mutex<TerminalSession>,
    pty: Mutex<Option<Arc<Pty>>>,
    kill: watch::Sender<bool>,
    /// The session task is running (the process may be alive).
    task_alive: AtomicBool,
    /// Held while a message is delivered, so two never interleave.
    typing: tokio::sync::Mutex<()>,
    /// How the harness confirms a typed message, and its readiness quiet.
    delivery: Mutex<(Expect, Duration)>,
    /// The question or dialog the session waits on.
    pending: Mutex<Option<SessionPrompt>>,
    /// Answers and "continue" for the session task.
    actions: mpsc::UnboundedSender<PromptAction>,
    actions_rx: Mutex<Option<mpsc::UnboundedReceiver<PromptAction>>>,
    /// The window of a session without a pty (chained headless, dsh).
    view: Mutex<Option<Arc<View>>>,
}

/// What the user decided about the session's pending prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAction {
    Answer {
        prompt_id: String,
        answer: PromptAnswer,
    },
    /// Mark the node waiting on a reply question done.
    Continue { node_id: String },
    /// A message for the node (chained sessions: typed in the window or sent
    /// from the Messages tab).
    Followup { node_id: String, text: String },
}

impl SessionShared {
    pub fn new(run_id: &str, graph_dir: &Path, info: TerminalSession) -> Arc<Self> {
        let (actions, actions_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            pending: Mutex::new(None),
            actions,
            actions_rx: Mutex::new(Some(actions_rx)),
            view: Mutex::new(None),
            run_id: run_id.to_string(),
            graph_dir: graph_dir.to_path_buf(),
            info: Mutex::new(info),
            pty: Mutex::new(None),
            kill: watch::channel(false).0,
            task_alive: AtomicBool::new(false),
            typing: tokio::sync::Mutex::new(()),
            delivery: Mutex::new((Expect::Submit, FALLBACK_QUIET)),
        })
    }

    /// The harness's submit check (set by the session task at its start).
    pub fn set_delivery(&self, expect: Expect, ready: Duration) {
        *lock(&self.delivery) = (expect, ready);
    }

    pub fn delivery(&self) -> (Expect, Duration) {
        lock(&self.delivery).clone()
    }

    /// The prompt the session waits on.
    pub fn pending(&self) -> Option<SessionPrompt> {
        lock(&self.pending).clone()
    }

    pub fn set_pending(&self, prompt: Option<SessionPrompt>) {
        *lock(&self.pending) = prompt;
    }

    /// Hand an answer to the session task.
    pub fn act(&self, action: PromptAction) -> AppResult<()> {
        self.actions
            .send(action)
            .map_err(|_| AppError::new("not_found", "the session has ended"))
    }

    pub fn set_view(&self, view: Arc<View>) {
        *lock(&self.view) = Some(view);
    }

    /// The window of a chained session (no pty).
    pub fn view(&self) -> Option<Arc<View>> {
        lock(&self.view).clone()
    }

    /// The node a window message goes to: the current one, else the last.
    pub fn message_node(&self) -> Option<String> {
        let info = self.info();
        info.current_node.or_else(|| info.node_ids.last().cloned())
    }

    /// The receiving end of [`SessionShared::act`] (once, for the task).
    pub fn take_actions(&self) -> Option<mpsc::UnboundedReceiver<PromptAction>> {
        lock(&self.actions_rx).take()
    }

    pub fn info(&self) -> TerminalSession {
        lock(&self.info).clone()
    }

    /// Apply `f` to the public state; returns the new state.
    pub fn update(&self, f: impl FnOnce(&mut TerminalSession)) -> TerminalSession {
        let mut info = lock(&self.info);
        f(&mut info);
        info.clone()
    }

    pub fn set_pty(&self, pty: Arc<Pty>) {
        *lock(&self.pty) = Some(pty);
    }

    pub fn pty(&self) -> Option<Arc<Pty>> {
        lock(&self.pty).clone()
    }

    /// Wait for the right to type into the session.
    pub async fn typing(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.typing.lock().await
    }

    pub fn kill_watch(&self) -> watch::Receiver<bool> {
        self.kill.subscribe()
    }

    /// Ask the session task to kill its process.
    pub fn request_kill(&self) {
        self.kill.send_replace(true);
    }

    pub fn task_started(&self) {
        self.task_alive.store(true, Ordering::SeqCst);
    }

    pub fn task_ended(&self) {
        self.task_alive.store(false, Ordering::SeqCst);
    }

    pub fn is_task_alive(&self) -> bool {
        self.task_alive.load(Ordering::SeqCst)
    }

    pub fn window(&self) -> WindowSpec {
        let info = self.info();
        WindowSpec {
            label: window_label(&self.run_id, &info.sid),
            title: format!("Claude Code — {}", info.title),
            url: format!(
                "index.html#/terminal/{}/{}?dir={}",
                self.run_id,
                info.sid,
                url_encode(&display_dir(&self.graph_dir))
            ),
            width: WINDOW_SIZE.0,
            height: WINDOW_SIZE.1,
            min_width: WINDOW_MIN.0,
            min_height: WINDOW_MIN.1,
        }
    }
}

/// Registry of terminal sessions by run id (sessions in sid order).
#[derive(Clone, Default)]
pub struct Terminals {
    runs: Arc<Mutex<BTreeMap<String, Vec<Arc<SessionShared>>>>>,
}

impl Terminals {
    pub fn insert(&self, session: Arc<SessionShared>) {
        lock(&self.runs)
            .entry(session.run_id.clone())
            .or_default()
            .push(session);
    }

    pub fn has_run(&self, run_id: &str) -> bool {
        lock(&self.runs).contains_key(run_id)
    }

    pub fn get(&self, run_id: &str, sid: &str) -> AppResult<Arc<SessionShared>> {
        lock(&self.runs)
            .get(run_id)
            .and_then(|v| v.iter().find(|s| s.info().sid == sid).cloned())
            .ok_or_else(|| AppError::new("not_found", format!("no session {sid} in run {run_id}")))
    }

    /// Every registered session of `run_id`.
    pub fn all_of(&self, run_id: &str) -> Vec<Arc<SessionShared>> {
        lock(&self.runs).get(run_id).cloned().unwrap_or_default()
    }

    pub fn sessions(&self, run_id: &str) -> Option<Vec<TerminalSession>> {
        lock(&self.runs)
            .get(run_id)
            .map(|v| v.iter().map(|s| s.info()).collect())
    }

    /// The live session of `run_id` that sends `node_id`.
    pub fn session_of_node(&self, run_id: &str, node_id: &str) -> Option<Arc<SessionShared>> {
        lock(&self.runs).get(run_id).and_then(|v| {
            v.iter()
                .find(|s| s.info().node_ids.iter().any(|n| n == node_id))
                .cloned()
        })
    }

    /// Kill every session of `run_id`; returns how many were asked.
    pub fn kill_run(&self, run_id: &str) -> usize {
        let sessions = lock(&self.runs).get(run_id).cloned().unwrap_or_default();
        sessions.iter().for_each(|s| s.request_kill());
        sessions.len()
    }

    /// Runs of the graph in `dir` with a session task still running, other
    /// than `except`.
    pub fn live_runs_of(&self, dir: &Path, except: &str) -> Vec<String> {
        lock(&self.runs)
            .iter()
            .filter(|(run, sessions)| {
                run.as_str() != except
                    && sessions
                        .iter()
                        .any(|s| s.graph_dir == dir && s.is_task_alive())
            })
            .map(|(run, _)| run.clone())
            .collect()
    }

    /// Kill every session (app exit); returns how many tasks were still alive.
    pub fn kill_all(&self) -> usize {
        let all: Vec<Arc<SessionShared>> = lock(&self.runs).values().flatten().cloned().collect();
        all.iter().for_each(|s| s.request_kill());
        all.iter().filter(|s| s.is_task_alive()).count()
    }

    /// Session tasks still running (their process may be alive).
    pub fn live_count(&self) -> usize {
        lock(&self.runs)
            .values()
            .flatten()
            .filter(|s| s.is_task_alive())
            .count()
    }

    /// Deliver `text` (one line) in the background into the session that
    /// sends `node_id` (follow-up message to a node of a terminal run).
    pub fn type_message(
        &self,
        run_id: &str,
        node_id: &str,
        text: &str,
        emitter: Arc<dyn Emitter>,
    ) -> AppResult<()> {
        let session = self
            .session_of_node(run_id, node_id)
            .ok_or_else(|| AppError::new("not_found", format!("node {node_id} has no session")))?;
        // A reply question of this node waits: the message is its answer.
        if let Some(p) = session.pending().filter(|p| p.node_id == node_id) {
            if let TurnPrompt::Reply(q) = &p.prompt {
                return session.act(PromptAction::Answer {
                    prompt_id: q.id.clone(),
                    answer: PromptAnswer::Text(text.to_string()),
                });
            }
        }
        if session.view().is_some() {
            return session.act(PromptAction::Followup {
                node_id: node_id.to_string(),
                text: text.to_string(),
            });
        }
        spawn_followup(session, emitter, node_id, text)
    }
}

/// Percent-encode everything but RFC 3986 unreserved characters.
pub fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

pub fn session_closed() -> AppError {
    AppError::new("not_found", "Session closed; rerun to talk")
}

/// `^s[0-9]+$`, at most 6 chars.
pub fn check_sid(sid: &str) -> AppResult<()> {
    let ok = sid.len() <= 6
        && sid.len() >= 2
        && sid.starts_with('s')
        && sid[1..].bytes().all(|b| b.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_input",
            format!("invalid session id {sid:?}"),
        ))
    }
}

fn checked(terminals: &Terminals, run_id: &str, sid: &str) -> AppResult<Arc<SessionShared>> {
    check_id("run id", run_id)?;
    check_sid(sid)?;
    terminals.get(run_id, sid)
}

/// Sessions of a run: live ones from the registry, else run.json's in `dir`
/// (`None`: the registry only, `not_found` when the run has no live sessions).
pub fn list_sessions(
    terminals: &Terminals,
    dir: Option<&Path>,
    run_id: &str,
) -> AppResult<Vec<TerminalSession>> {
    check_id("run id", run_id)?;
    match (terminals.sessions(run_id), dir) {
        (Some(live), _) => Ok(live),
        (None, Some(dir)) => Ok(read_run_state(dir, run_id)?.sessions),
        (None, None) => Err(AppError::new(
            "not_found",
            format!("run {run_id} has no live sessions"),
        )),
    }
}

pub fn pty_write(terminals: &Terminals, run_id: &str, sid: &str, text: &str) -> AppResult<()> {
    if text.len() > WRITE_MAX {
        return Err(AppError::new(
            "invalid_input",
            format!("text is {} bytes; at most {WRITE_MAX}", text.len()),
        ));
    }
    let session = checked(terminals, run_id, sid)?;
    if text.is_empty() {
        return Ok(());
    }
    if let (None, Some(view)) = (session.pty(), session.view()) {
        // A chained session: a typed line is a message to its node.
        let node_id = session.message_node().ok_or_else(session_closed)?;
        for line in view.keys(text) {
            if !session.is_task_alive() {
                return Err(session_closed());
            }
            if let Some(p) = session.pending().filter(|p| p.node_id == node_id) {
                session.act(PromptAction::Answer {
                    prompt_id: p.prompt.id().to_string(),
                    answer: PromptAnswer::Text(line),
                })?;
            } else {
                session.act(PromptAction::Followup {
                    node_id: node_id.clone(),
                    text: line,
                })?;
            }
        }
        return Ok(());
    }
    let pty = session.pty().ok_or_else(session_closed)?;
    pty.write(text.as_bytes())
}

pub fn pty_resize(
    terminals: &Terminals,
    run_id: &str,
    sid: &str,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    let in_range = |v: u16, (lo, hi): (u16, u16)| (lo..=hi).contains(&v);
    if !in_range(cols, COLS_RANGE) || !in_range(rows, ROWS_RANGE) {
        return Err(AppError::new(
            "invalid_input",
            format!("terminal size {cols}x{rows} is out of range"),
        ));
    }
    let session = checked(terminals, run_id, sid)?;
    if session.pty().is_none() && session.view().is_some() {
        return Ok(()); // a chained session's window has no process to resize
    }
    let pty = session.pty().ok_or_else(session_closed)?;
    pty.resize(cols, rows)
}

/// The output so far (last 512 KiB) and its chunk count; empty before the
/// process started.
pub fn pty_replay(terminals: &Terminals, run_id: &str, sid: &str) -> AppResult<PtyReplay> {
    let session = checked(terminals, run_id, sid)?;
    Ok(session
        .pty()
        .map(|p| p.output.replay())
        .or_else(|| session.view().map(|v| v.output.replay()))
        .unwrap_or(PtyReplay {
            text: String::new(),
            seq: 0,
        }))
}

pub fn open_session_window(
    terminals: &Terminals,
    emitter: &dyn Emitter,
    run_id: &str,
    sid: &str,
) -> AppResult<()> {
    let session = checked(terminals, run_id, sid)?;
    emitter.open_window(&session.window())
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;

//! Active runs: start, cancel, follow-up messages, the stale-run sweep and
//! app-exit shutdown. Used by the Tauri commands and directly by the
//! end-to-end tests (no Tauri needed).
//!
//! One run per graph folder: the folder is reserved under the registry lock
//! before any I/O (a placeholder entry, released on error), so two concurrent
//! `start_run` calls cannot both pass the check. A follow-up to a finished run
//! starts a "resumed" loop under the same rule.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::app_settings::{self, default_data_dir};
use crate::assistant::AssistantTurns;
use crate::claude_bin::{resolve, ClaudeCommand, Resolution, ResolveEnv};
use crate::contract::{
    AppError, AppResult, AppSettings, HarnessKind, HarnessProfile, RunMode, RunState, RunTarget,
    TrustStatus,
};
use crate::events::Emitter;
use crate::harness;
use crate::run_loop::{initial_state, LoopMsg, RunLoop};
use crate::run_store::{check_id, list_run_states, new_run_id, read_run_state, sweep_run};
use crate::scheduler::Scheduler;
use crate::store::read_graph;
use crate::terminal::api::session_closed;
use crate::terminal::components::Labels;
use crate::terminal::run::TerminalLoop;
use crate::terminal::{plan, Terminals};
use crate::trust;
use crate::turns::{checked_session, RunCtx};

pub use crate::paths::abs;

/// How long a follow-up waits for another follow-up that is resuming the same run.
const RESUME_WAIT: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct RunHandle {
    graph_dir: PathBuf,
    tx: mpsc::UnboundedSender<LoopMsg>,
}

#[derive(Default)]
struct Registry {
    /// Live loops by run id.
    runs: HashMap<String, RunHandle>,
    /// Graph folders being set up (placeholder), with the run id for a resume.
    reserved: HashMap<PathBuf, Option<String>>,
}

impl Registry {
    fn owns_dir(&self, dir: &Path) -> bool {
        self.reserved.contains_key(dir) || self.runs.values().any(|h| h.graph_dir == dir)
    }

    fn is_live(&self, run_id: &str) -> bool {
        self.runs.contains_key(run_id)
            || self.reserved.values().any(|r| r.as_deref() == Some(run_id))
    }
}

/// Tauri-managed state: the runs whose loop is alive, and where the app keeps
/// its settings and trust records.
#[derive(Clone)]
pub struct AppState {
    registry: Arc<Mutex<Registry>>,
    /// Running assistant-mode scheduler turns (see [`crate::assistant::api`]).
    assistant: AssistantTurns,
    /// Terminal-mode sessions, alive or not, of this app run.
    terminals: Terminals,
    data_dir: PathBuf,
    /// The program Codex's notify runs (`<exe> --notify <hooks.log>`): this
    /// app's binary.
    notify_exe: PathBuf,
}

impl Default for AppState {
    /// Uses `<data_dir>/claude-graph` (temp dir if the OS has none).
    fn default() -> Self {
        Self::new(default_data_dir().unwrap_or_else(|| std::env::temp_dir().join("claude-graph")))
    }
}

/// A graph folder reserved for a run being set up; released on drop unless
/// [`AppState::commit`] turned it into a live run.
struct Reservation {
    state: AppState,
    dir: PathBuf,
    armed: bool,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.armed {
            self.state.lock().reserved.remove(&self.dir);
        }
    }
}

/// Removes the registry entry when the loop task ends, also by panic.
struct Deregister {
    state: AppState,
    run_id: String,
}

impl Drop for Deregister {
    fn drop(&mut self) {
        self.state.lock().runs.remove(&self.run_id);
    }
}

enum Claim {
    Reserved(Reservation),
    Active(RunHandle),
    /// Another follow-up is resuming this very run right now.
    Starting,
    Busy,
}

fn run_active() -> AppError {
    AppError::new(
        "run_active",
        "a run of this graph is already active; cancel it first",
    )
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
            assistant: AssistantTurns::default(),
            terminals: Terminals::default(),
            data_dir,
            notify_exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("claude-graph")),
        }
    }

    /// Use `exe` as Codex's notify program (tests: the built app binary).
    pub fn with_notify_exe(self, exe: PathBuf) -> Self {
        Self {
            notify_exe: exe,
            ..self
        }
    }

    /// Harness profiles of this machine (built-ins with overrides, customs).
    pub fn harnesses(&self) -> Vec<HarnessProfile> {
        harness::store::list(&self.data_dir)
    }

    pub fn save_harness(&self, profile: &HarnessProfile) -> AppResult<Vec<HarnessProfile>> {
        harness::store::save(&self.data_dir, profile)
    }

    pub fn delete_harness(&self, id: &str) -> AppResult<Vec<HarnessProfile>> {
        harness::store::delete(&self.data_dir, id)
    }

    /// The claude command when some session needs it; otherwise a missing
    /// claude is not an error (a Codex-only run) and a placeholder is used.
    fn claude_for(&self, needed: bool) -> AppResult<ClaudeCommand> {
        match self.find_claude() {
            Ok(c) => Ok(c),
            Err(e) if needed => Err(e),
            Err(_) => Ok(ClaudeCommand {
                program: PathBuf::from("claude"),
                prefix_args: Vec::new(),
                display: "claude (not found)".into(),
            }),
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub(crate) fn assistant_turns(&self) -> &AssistantTurns {
        &self.assistant
    }

    /// Terminal-mode sessions (live registry).
    pub fn terminals(&self) -> &Terminals {
        &self.terminals
    }

    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn handle(&self, run_id: &str) -> Option<RunHandle> {
        self.lock().runs.get(run_id).cloned()
    }

    pub fn is_active(&self, run_id: &str) -> bool {
        self.lock().runs.contains_key(run_id)
    }

    pub fn active_count(&self) -> usize {
        self.lock().runs.len()
    }

    pub fn app_settings(&self) -> AppSettings {
        app_settings::load(&self.data_dir)
    }

    pub fn set_app_settings(&self, settings: &AppSettings) -> AppResult<AppSettings> {
        app_settings::save(&self.data_dir, settings)
    }

    /// The CLI per the resolution order (app settings, env, ~/.local/bin, PATH).
    pub fn find_claude(&self) -> AppResult<ClaudeCommand> {
        let env = ResolveEnv::from_process(self.app_settings().claude_bin);
        match resolve(&env) {
            Resolution::Found(c) => Ok(c),
            Resolution::Missing { tried } => Err(AppError::new(
                "claude_not_found",
                match tried {
                    Some(t) => format!("claude CLI not found at {t}"),
                    None => "claude CLI not found; install Claude Code or set its path in the app settings"
                        .to_string(),
                },
            )),
        }
    }

    /// Trust status of the graph saved in canonical folder `dir`.
    pub fn trust_status(&self, dir: &Path) -> AppResult<TrustStatus> {
        let graph = read_graph(dir)?;
        Ok(trust::status(&self.data_dir, dir, &graph))
    }

    /// Accept the current elevated settings of the graph saved in `dir`.
    pub fn trust_graph(&self, dir: &Path) -> AppResult<()> {
        let graph = read_graph(dir)?;
        trust::trust(&self.data_dir, dir, &graph)
    }

    fn reserve(&self, dir: &Path) -> AppResult<Reservation> {
        let mut reg = self.lock();
        if reg.owns_dir(dir) {
            return Err(run_active());
        }
        reg.reserved.insert(dir.to_path_buf(), None);
        Ok(Reservation {
            state: self.clone(),
            dir: dir.to_path_buf(),
            armed: true,
        })
    }

    fn claim_resume(&self, dir: &Path, run_id: &str) -> Claim {
        let mut reg = self.lock();
        if let Some(h) = reg.runs.get(run_id) {
            return Claim::Active(h.clone());
        }
        if reg.runs.values().any(|h| h.graph_dir == dir) {
            return Claim::Busy;
        }
        match reg.reserved.get(dir) {
            Some(Some(r)) if r == run_id => Claim::Starting,
            Some(_) => Claim::Busy,
            None => {
                reg.reserved
                    .insert(dir.to_path_buf(), Some(run_id.to_string()));
                Claim::Reserved(Reservation {
                    state: self.clone(),
                    dir: dir.to_path_buf(),
                    armed: true,
                })
            }
        }
    }

    /// Turn a reservation into a live run and spawn its loop.
    fn commit(
        &self,
        reservation: Reservation,
        run_loop: RunLoop,
        run_id: String,
    ) -> JoinHandle<RunState> {
        let tx = run_loop.sender();
        self.commit_with(reservation, tx, run_loop.run(), run_id)
    }

    /// [`Self::commit`] for any loop future fed by `tx`.
    fn commit_with(
        &self,
        mut reservation: Reservation,
        tx: mpsc::UnboundedSender<LoopMsg>,
        run: impl std::future::Future<Output = RunState> + Send + 'static,
        run_id: String,
    ) -> JoinHandle<RunState> {
        let handle = RunHandle {
            graph_dir: reservation.dir.clone(),
            tx,
        };
        {
            let mut reg = self.lock();
            reg.reserved.remove(&reservation.dir);
            reg.runs.insert(run_id.clone(), handle);
        }
        reservation.armed = false;
        let guard = Deregister {
            state: self.clone(),
            run_id,
        };
        tokio::spawn(async move {
            let _guard = guard;
            run.await
        })
    }

    /// The `start_run` command: a terminal or headless run per the graph's
    /// `settings.run_mode`.
    pub fn start_by_mode(
        &self,
        dir: &Path,
        target: RunTarget,
        emitter: Arc<dyn Emitter>,
    ) -> AppResult<(String, JoinHandle<RunState>)> {
        match read_graph(&abs(dir))?.settings.run_mode {
            RunMode::Terminal => self.start_terminal_run(dir, target, emitter),
            RunMode::Headless => self.start_run(dir, target, emitter),
        }
    }

    /// Start a terminal-mode run (one Claude Code window per connected
    /// component), whatever `settings.run_mode` says. Returns at once; the
    /// join handle ends when every session is done (processes stay alive).
    pub fn start_terminal_run(
        &self,
        dir: &Path,
        target: RunTarget,
        emitter: Arc<dyn Emitter>,
    ) -> AppResult<(String, JoinHandle<RunState>)> {
        let graph_dir = abs(dir);
        let reservation = self.reserve(&graph_dir)?;
        let graph = read_graph(&graph_dir)?;
        trust::require_trusted(&self.data_dir, &graph_dir, &graph)?;
        let harnesses = self.harnesses();
        let labels: Labels = harnesses
            .iter()
            .map(|h| (h.id.clone(), h.label.clone()))
            .collect();
        let plans = plan(&graph, &target, &list_run_states(&graph_dir)?, &labels)?;
        let targets = plans.iter().flat_map(|p| p.to_send());
        harness::check_targets(&graph, targets, &harnesses, false)?;
        let needs_claude = plans.iter().filter(|p| !p.to_send().is_empty()).any(|p| {
            harness::find(&harnesses, &p.harness)
                .is_some_and(|h| h.kind == HarnessKind::Claude && h.command.is_none())
        });
        let claude = self.claude_for(needs_claude)?;
        let run_id = new_run_id(&graph_dir, Utc::now())?;
        let ctx = Arc::new(RunCtx {
            graph_dir,
            graph,
            run_id: run_id.clone(),
            claude,
            emitter,
            harnesses,
            notify_exe: self.notify_exe.clone(),
        });
        self.close_earlier_sessions(&ctx, &plans);
        let run_loop = TerminalLoop::new(ctx, plans, self.terminals.clone());
        let tx = run_loop.sender();
        let join = self.commit_with(reservation, tx, run_loop.run(), run_id.clone());
        Ok((run_id, join))
    }

    /// A new terminal run of a graph first kills the sessions of its earlier
    /// runs and says so with a `text` node event on the new run.
    fn close_earlier_sessions(&self, ctx: &RunCtx, plans: &[crate::terminal::SessionPlan]) {
        let node_id = plans
            .iter()
            .find_map(|p| p.to_send().first().cloned())
            .unwrap_or_default();
        for old in self.terminals.live_runs_of(&ctx.graph_dir, &ctx.run_id) {
            self.terminals.kill_run(&old);
            ctx.emitter.node_event(&crate::contract::NodeEvent {
                run_id: ctx.run_id.clone(),
                node_id: node_id.clone(),
                kind: crate::contract::NodeEventKind::Text,
                text: format!("closed sessions of run {old}"),
                ts: Utc::now(),
            });
        }
    }

    /// Start a headless (`claude -p`) run, whatever `settings.run_mode` says;
    /// returns at once with its id and the loop's join handle.
    /// `dir` should already be canonical ([`crate::paths::existing_dir`]).
    pub fn start_run(
        &self,
        dir: &Path,
        target: RunTarget,
        emitter: Arc<dyn Emitter>,
    ) -> AppResult<(String, JoinHandle<RunState>)> {
        let graph_dir = abs(dir);
        let reservation = self.reserve(&graph_dir)?;
        let graph = read_graph(&graph_dir)?;
        trust::require_trusted(&self.data_dir, &graph_dir, &graph)?;
        let claude = self.find_claude()?;
        let sched = Scheduler::new(&graph, &target, &list_run_states(&graph_dir)?)?;
        let harnesses = self.harnesses();
        harness::check_targets(&graph, sched.target(), &harnesses, true)?;
        let run_id = new_run_id(&graph_dir, Utc::now())?;
        let ctx = Arc::new(RunCtx {
            graph_dir,
            graph,
            run_id: run_id.clone(),
            claude,
            emitter,
            harnesses,
            notify_exe: self.notify_exe.clone(),
        });
        let state = initial_state(&ctx, &sched);
        let run_loop = RunLoop::new(ctx, sched, state);
        let join = self.commit(reservation, run_loop, run_id.clone());
        Ok((run_id, join))
    }

    fn send(&self, run_id: &str, msg: LoopMsg) -> AppResult<()> {
        let handle = self
            .handle(run_id)
            .ok_or_else(|| AppError::new("not_found", format!("run {run_id} is not active")))?;
        handle
            .tx
            .send(msg)
            .map_err(|_| AppError::new("not_found", format!("run {run_id} has finished")))
    }

    /// Cancel an active run. A finished terminal run's sessions that are
    /// still alive are killed too.
    pub fn cancel_run(&self, run_id: &str) -> AppResult<()> {
        match self.send(run_id, LoopMsg::CancelRun) {
            Err(_) if self.terminals.has_run(run_id) => {
                self.terminals.kill_run(run_id);
                Ok(())
            }
            other => other,
        }
    }

    /// Cancel one node; in terminal mode this kills the node's session.
    pub fn cancel_node(&self, run_id: &str, node_id: &str) -> AppResult<()> {
        match self.send(run_id, LoopMsg::CancelNode(node_id.to_string())) {
            Err(e) => match self.terminals.session_of_node(run_id, node_id) {
                Some(session) => {
                    session.request_kill();
                    Ok(())
                }
                None => Err(e),
            },
            ok => ok,
        }
    }

    /// App exit: cancel every active run and assistant turn and wait up to
    /// `timeout` for them to finish (they kill their process trees and write
    /// run.json / session.json). Returns how many were still alive at the deadline.
    pub async fn shutdown(&self, timeout: Duration) -> usize {
        let handles: Vec<(String, RunHandle)> = self
            .lock()
            .runs
            .iter()
            .map(|(id, h)| (id.clone(), h.clone()))
            .collect();
        for (id, h) in &handles {
            if h.tx.send(LoopMsg::CancelRun).is_err() {
                log::warn!("shutdown: run {id} already finished");
            }
        }
        let turns = self.assistant.kill_all();
        if turns > 0 {
            log::info!("shutdown: cancelling {turns} assistant turn(s)");
        }
        let sessions = self.terminals.kill_all();
        if sessions > 0 {
            log::info!("shutdown: killing {sessions} terminal session(s)");
        }
        let alive =
            || self.active_count() + self.assistant.live_count() + self.terminals.live_count();
        let deadline = Instant::now() + timeout;
        while alive() > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let left = alive();
        if left > 0 {
            log::warn!(
                "shutdown: {left} run loop(s) / assistant turn(s) still alive after {timeout:?}"
            );
        }
        left
    }

    /// Rewrite every run of `dir` that run.json says is `running` but that has
    /// no live loop (the app exited or crashed) as cancelled / interrupted.
    /// Returns the swept run ids. Errors on single runs are logged.
    pub fn sweep_stale(&self, dir: &Path) -> AppResult<Vec<String>> {
        let running: Vec<String> = list_run_states(dir)?
            .into_iter()
            .filter(|r| r.status == crate::contract::RunStatus::Running)
            .map(|r| r.run_id)
            .collect();
        let mut swept = Vec::new();
        for run_id in running {
            // Holding the registry: no loop for this run can start or end meanwhile.
            let reg = self.lock();
            if reg.is_live(&run_id) {
                continue;
            }
            match sweep_run(dir, &run_id) {
                Ok(true) => swept.push(run_id),
                Ok(false) => {}
                Err(e) => log::warn!("sweeping run {run_id} failed: {e}"),
            }
            drop(reg);
        }
        Ok(swept)
    }

    /// Follow-up message to a node. In an active run the loop handles it; in a
    /// finished run a resumed loop is started for it, whose join handle is returned.
    pub async fn send_message(
        &self,
        dir: &Path,
        run_id: &str,
        node_id: &str,
        text: &str,
        emitter: Arc<dyn Emitter>,
    ) -> AppResult<Option<JoinHandle<RunState>>> {
        check_id("run id", run_id)?;
        check_id("node id", node_id)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(AppError::new("invalid_input", "message is empty"));
        }
        let graph_dir = abs(dir);
        if self.terminals.has_run(run_id) {
            // Terminal mode: deliver the message into the node's session.
            self.terminals
                .type_message(run_id, node_id, text, emitter.clone())?;
            return Ok(None);
        }
        let graph = read_graph(&graph_dir)?;
        trust::require_trusted(&self.data_dir, &graph_dir, &graph)?;
        let started = Instant::now();
        let reservation = loop {
            match self.claim_resume(&graph_dir, run_id) {
                Claim::Reserved(r) => break r,
                Claim::Active(handle) => {
                    if handle.graph_dir != graph_dir {
                        return Err(AppError::new(
                            "not_found",
                            format!("run {run_id} belongs to another graph"),
                        ));
                    }
                    match deliver(&handle, node_id, text).await {
                        Some(result) => return result.map(|_| None),
                        // The loop ended between lookup and delivery: try again.
                        None => continue,
                    }
                }
                Claim::Starting if started.elapsed() < RESUME_WAIT => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Claim::Starting | Claim::Busy => return Err(run_active()),
            }
        };
        let state = read_run_state(&graph_dir, run_id)?;
        if state.mode == RunMode::Terminal {
            return Err(session_closed());
        }
        if state.run_id != run_id {
            return Err(AppError::new("invalid_input", "run.json names another run"));
        }
        let node = state.nodes.get(node_id).ok_or_else(|| {
            AppError::new("not_found", format!("node {node_id} is not in this run"))
        })?;
        match node.session_id.as_deref() {
            None => return Err(AppError::new("not_found", "node has not run yet")),
            Some(sid) => {
                checked_session(sid)?;
            }
        }
        let claude = self.find_claude()?;
        let sched = Scheduler::resume(&graph, &state)?;
        let ctx = Arc::new(RunCtx {
            graph_dir,
            graph,
            run_id: run_id.to_string(),
            claude,
            emitter,
            harnesses: self.harnesses(),
            notify_exe: self.notify_exe.clone(),
        });
        let run_loop = RunLoop::new(ctx, sched, state);
        // Queue the message before the loop starts so it cannot finish first.
        let (reply_tx, reply_rx) = oneshot::channel();
        run_loop
            .sender()
            .send(LoopMsg::SendMessage {
                node_id: node_id.to_string(),
                text: text.to_string(),
                reply: reply_tx,
            })
            .map_err(|_| AppError::new("internal", "run loop closed"))?;
        let join = self.commit(reservation, run_loop, run_id.to_string());
        match reply_rx.await {
            Ok(result) => result.map(|_| Some(join)),
            Err(_) => Err(AppError::new("internal", "run loop closed")),
        }
    }
}

/// `None` when the loop is gone before it answered.
async fn deliver(handle: &RunHandle, node_id: &str, text: &str) -> Option<AppResult<()>> {
    let (reply_tx, reply_rx) = oneshot::channel();
    let msg = LoopMsg::SendMessage {
        node_id: node_id.to_string(),
        text: text.to_string(),
        reply: reply_tx,
    };
    handle.tx.send(msg).ok()?;
    reply_rx.await.ok()
}

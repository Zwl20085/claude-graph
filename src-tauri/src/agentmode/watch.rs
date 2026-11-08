//! The GUI side: while a folder is open, a background thread polls its
//! newest agent run every second (run.json size and mtime, the run log) and
//! pushes changes to the window as `run:state`, `node:event` and
//! `agent_run` events, so the canvas follows a CLI-driven run within ~1 s.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use crate::contract::{NodeEvent, NodeEventKind, RunState, RunStatus};
use crate::contract_agent::AgentRunInfo;
use crate::prompt::display_dir;
use crate::run_store::{run_dir, RUN_FILE};

use super::run::{self, LogEntry};

/// Poll period of the watcher thread.
pub const POLL: Duration = Duration::from_secs(1);

/// What changed since the previous poll.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub info: AgentRunInfo,
    /// Set when run.json changed.
    pub state: Option<RunState>,
    pub events: Vec<NodeEvent>,
}

/// Polls one folder (pure state machine over the files; unit-tested).
#[derive(Debug, Clone)]
pub struct Poller {
    dir: PathBuf,
    /// (run id, run.json mtime, size) last seen.
    seen: Option<(String, Option<SystemTime>, u64)>,
    log_offset: u64,
    last_note: Option<String>,
}

fn kind_of(entry: &LogEntry) -> NodeEventKind {
    match entry.kind.as_str() {
        "report" => NodeEventKind::Result,
        "handout" | "run" => NodeEventKind::System,
        _ => NodeEventKind::Text,
    }
}

impl Poller {
    /// Starts from what is on disk now: only later changes are pushed (the
    /// window loads the newest run itself, which may be a newer app run).
    pub fn new(dir: PathBuf) -> Self {
        let mut poller = Self {
            dir,
            seen: None,
            log_offset: 0,
            last_note: None,
        };
        if poller.poll().is_some() {
            log::debug!("agent watcher primed on {}", poller.dir.display());
        }
        poller
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn info(&self, run: &run::AgentRun) -> AgentRunInfo {
        AgentRunInfo {
            dir: display_dir(&self.dir),
            run_id: run.state.run_id.clone(),
            client: run.meta.client.clone(),
            status: run.state.status,
            active: run.state.status == RunStatus::Running,
            last_seen: run.meta.last_seen,
            last_note: self.last_note.clone(),
        }
    }

    /// The current agent run, without consuming changes.
    pub fn current(&self) -> Option<AgentRunInfo> {
        run::latest(&self.dir).map(|r| self.info(&r))
    }

    /// One poll: `Some` when the newest agent run or its log changed.
    pub fn poll(&mut self) -> Option<Update> {
        let found = run::latest(&self.dir)?;
        let id = found.state.run_id.clone();
        let meta = std::fs::metadata(run_dir(&self.dir, &id).join(RUN_FILE)).ok();
        let stamp = (
            id.clone(),
            meta.as_ref().and_then(|m| m.modified().ok()),
            meta.as_ref().map_or(0, |m| m.len()),
        );
        let new_run = self.seen.as_ref().map(|s| &s.0) != Some(&id);
        if new_run {
            self.log_offset = 0;
            self.last_note = None;
        }
        let changed = self.seen.as_ref() != Some(&stamp);
        let (entries, offset) = run::read_log(&self.dir, &id, self.log_offset);
        self.log_offset = offset;
        self.seen = Some(stamp);
        if let Some(note) = entries
            .iter()
            .rev()
            .find(|e| e.node_id.is_none() && e.kind == "note")
        {
            self.last_note = Some(note.text.clone());
        }
        if !changed && entries.is_empty() {
            return None;
        }
        let events = entries
            .iter()
            .filter_map(|e| {
                let node_id = e.node_id.clone()?;
                Some(NodeEvent {
                    run_id: id.clone(),
                    node_id,
                    kind: kind_of(e),
                    text: e.text.clone(),
                    ts: e.ts,
                })
            })
            .collect();
        Some(Update {
            info: self.info(&found),
            state: changed.then(|| found.state.clone()),
            events,
        })
    }
}

/// Where updates go (the Tauri emitter in the app; a recorder in tests).
pub trait Sink: Send + Sync {
    fn update(&self, update: &Update);
}

#[derive(Default)]
struct State {
    poller: Option<Poller>,
    started: bool,
}

/// The app's watcher (managed Tauri state; cheap to clone).
#[derive(Clone, Default)]
pub struct AgentWatch {
    inner: Arc<Mutex<State>>,
}

impl AgentWatch {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Watch `dir` (`None`: stop watching); returns its current agent run.
    /// The thread starts on first use and lives as long as the app.
    pub fn watch(&self, dir: Option<PathBuf>, sink: Arc<dyn Sink>) -> Option<AgentRunInfo> {
        let poller = dir.map(Poller::new);
        let info = poller.as_ref().and_then(Poller::current);
        let start = {
            let mut st = self.lock();
            let same = match (&st.poller, &poller) {
                (Some(a), Some(b)) => a.dir() == b.dir(),
                _ => false,
            };
            if !same {
                st.poller = poller;
            }
            let start = !st.started;
            st.started = true;
            start
        };
        if start {
            let inner = Arc::downgrade(&self.inner);
            let spawned = std::thread::Builder::new()
                .name("agent-watch".into())
                .spawn(move || loop {
                    std::thread::sleep(POLL);
                    let Some(inner) = inner.upgrade() else { return };
                    let update = {
                        let mut st = inner.lock().unwrap_or_else(|p| p.into_inner());
                        st.poller.as_mut().and_then(Poller::poll)
                    };
                    if let Some(u) = update {
                        sink.update(&u);
                    }
                });
            if let Err(e) = spawned {
                log::error!("agent watcher could not start: {e}");
                self.lock().started = false;
            }
        }
        info
    }

    /// The watched folder, if any.
    pub fn dir(&self) -> Option<PathBuf> {
        self.lock().poller.as_ref().map(|p| p.dir().to_path_buf())
    }
}

/// `cancel_run` for a run id the app's registry does not know: the agent run
/// of the watched folder is stopped. `None` when it is not one.
pub fn cancel_agent_run(
    watch: &AgentWatch,
    run_id: &str,
) -> Option<crate::contract::AppResult<()>> {
    let dir = watch.dir()?;
    if !run::is_agent_run(&dir, run_id) {
        return None;
    }
    Some(super::open::abandon(&dir, run_id).map(|_| ()))
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;

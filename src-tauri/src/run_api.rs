//! Active runs: start, cancel, follow-up messages. Used by the Tauri commands
//! and directly by the end-to-end tests (no Tauri needed).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::claude_bin::{resolve, ClaudeCommand, Resolution, ResolveEnv};
use crate::contract::{AppError, AppResult, Graph, RunState, RunTarget};
use crate::events::Emitter;
use crate::run_loop::{initial_state, LoopMsg, RunLoop};
use crate::run_store::{check_id, list_run_states, new_run_id, read_run_state};
use crate::scheduler::Scheduler;
use crate::store::read_graph;
use crate::turns::RunCtx;

#[derive(Clone)]
struct RunHandle {
    graph_dir: PathBuf,
    tx: mpsc::UnboundedSender<LoopMsg>,
}

/// Tauri-managed state: the runs whose loop is alive, by run id.
#[derive(Clone, Default)]
pub struct AppState {
    runs: Arc<Mutex<HashMap<String, RunHandle>>>,
}

/// The graph folder in long, absolute form. On Windows an 8.3 short segment
/// (`C:\Users\ZHANGW~1\…`, common in `%TEMP%`) makes Claude Code refuse
/// writes as a "suspicious Windows path", so the path is canonicalised and the
/// verbatim `\\?\` prefix dropped again for a plain drive path.
pub fn abs(dir: &Path) -> PathBuf {
    let plain = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    match std::fs::canonicalize(&plain) {
        Ok(long) => strip_verbatim(long).unwrap_or(plain),
        Err(_) => plain,
    }
}

/// `\\?\C:\x` -> `C:\x`; `None` for verbatim UNC or other forms we keep as given.
fn strip_verbatim(path: PathBuf) -> Option<PathBuf> {
    let s = path.to_str()?;
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => Some(PathBuf::from(rest)),
        Some(_) => None,
        None => Some(path),
    }
}

fn find_claude(graph: &Graph) -> AppResult<ClaudeCommand> {
    match resolve(&ResolveEnv::from_process(graph.settings.claude_bin.clone())) {
        Resolution::Found(c) => Ok(c),
        Resolution::Missing { tried } => Err(AppError::new(
            "claude_not_found",
            match tried {
                Some(t) => format!("claude CLI not found at {t}"),
                None => "claude CLI not found; install Claude Code or set its path in settings"
                    .to_string(),
            },
        )),
    }
}

impl AppState {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, RunHandle>> {
        self.runs.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn handle(&self, run_id: &str) -> Option<RunHandle> {
        self.lock().get(run_id).cloned()
    }

    pub fn is_active(&self, run_id: &str) -> bool {
        self.lock().contains_key(run_id)
    }

    /// Register the loop and spawn it; the entry is removed when the loop ends.
    fn launch(
        &self,
        run_loop: RunLoop,
        run_id: String,
        graph_dir: PathBuf,
    ) -> JoinHandle<RunState> {
        let handle = RunHandle {
            graph_dir,
            tx: run_loop.sender(),
        };
        self.lock().insert(run_id.clone(), handle);
        self.spawn_loop(run_loop, run_id)
    }

    fn spawn_loop(&self, run_loop: RunLoop, run_id: String) -> JoinHandle<RunState> {
        let registry = self.clone();
        tokio::spawn(async move {
            let final_state = run_loop.run().await;
            registry.lock().remove(&run_id);
            final_state
        })
    }

    /// Start a run; returns at once with its id and the loop's join handle.
    pub fn start_run(
        &self,
        dir: &Path,
        target: RunTarget,
        emitter: Arc<dyn Emitter>,
    ) -> AppResult<(String, JoinHandle<RunState>)> {
        let graph_dir = abs(dir);
        if self.lock().values().any(|h| h.graph_dir == graph_dir) {
            return Err(AppError::new(
                "run_active",
                "a run of this graph is already active; cancel it first",
            ));
        }
        let graph = read_graph(&graph_dir)?;
        let claude = find_claude(&graph)?;
        let sched = Scheduler::new(&graph, &target, &list_run_states(&graph_dir)?)?;
        let run_id = new_run_id(&graph_dir, Utc::now())?;
        let ctx = Arc::new(RunCtx {
            graph_dir: graph_dir.clone(),
            graph,
            run_id: run_id.clone(),
            claude,
            emitter,
        });
        let state = initial_state(&ctx, &sched);
        let run_loop = RunLoop::new(ctx, sched, state);
        let join = self.launch(run_loop, run_id.clone(), graph_dir);
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

    pub fn cancel_run(&self, run_id: &str) -> AppResult<()> {
        self.send(run_id, LoopMsg::CancelRun)
    }

    pub fn cancel_node(&self, run_id: &str, node_id: &str) -> AppResult<()> {
        self.send(run_id, LoopMsg::CancelNode(node_id.to_string()))
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
        if let Some(handle) = self.handle(run_id) {
            if let Some(result) = deliver(&handle, node_id, text).await {
                return result.map(|_| None);
            }
            // The loop ended between lookup and delivery: fall through.
        }
        let graph_dir = abs(dir);
        let state = read_run_state(&graph_dir, run_id)?;
        let node = state.nodes.get(node_id).ok_or_else(|| {
            AppError::new("not_found", format!("node {node_id} is not in this run"))
        })?;
        if node.session_id.is_none() {
            return Err(AppError::new("not_found", "node has not run yet"));
        }
        let graph = read_graph(&graph_dir)?;
        let claude = find_claude(&graph)?;
        let sched = Scheduler::resume(&graph, &state)?;
        let ctx = Arc::new(RunCtx {
            graph_dir: graph_dir.clone(),
            graph,
            run_id: run_id.to_string(),
            claude,
            emitter,
        });
        let run_loop = RunLoop::new(ctx, sched, state);
        let handle = RunHandle {
            graph_dir: graph_dir.clone(),
            tx: run_loop.sender(),
        };
        // Queue the message before the loop starts so it cannot finish first.
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .tx
            .send(LoopMsg::SendMessage {
                node_id: node_id.to_string(),
                text: text.to_string(),
                reply: reply_tx,
            })
            .map_err(|_| AppError::new("internal", "run loop closed"))?;
        let registered = {
            let mut runs = self.lock();
            let free = !runs.contains_key(run_id);
            if free {
                runs.insert(run_id.to_string(), handle);
            }
            free
        };
        if !registered {
            // Another message resumed this run concurrently: hand it over.
            let other = self
                .handle(run_id)
                .ok_or_else(|| AppError::new("internal", "run loop closed"))?;
            return deliver(&other, node_id, text)
                .await
                .unwrap_or_else(|| Err(AppError::new("internal", "run loop closed")))
                .map(|_| None);
        }
        let join = self.spawn_loop(run_loop, run_id.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_verbatim_keeps_plain_drive_paths() {
        let verbatim = format!("{}{}", r"\\?\", r"C:\Users\x");
        assert_eq!(
            strip_verbatim(PathBuf::from(verbatim)),
            Some(PathBuf::from(r"C:\Users\x"))
        );
        let unc = format!("{}{}", r"\\?\", r"UNC\host\share");
        assert_eq!(strip_verbatim(PathBuf::from(unc)), None);
        assert_eq!(
            strip_verbatim(PathBuf::from("/work/g")),
            Some(PathBuf::from("/work/g"))
        );
    }

    #[test]
    fn abs_gives_an_existing_absolute_long_path() {
        let tmp = tempfile::tempdir().unwrap();
        let got = abs(tmp.path());
        assert!(got.is_absolute() && got.is_dir());
        assert!(!got.to_string_lossy().starts_with(r"\\?\"));
        assert!(!got.to_string_lossy().contains('~'), "{}", got.display());
    }
}

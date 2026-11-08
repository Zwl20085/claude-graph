//! One terminal session: spawn Claude Code in a pty, wait for it to start,
//! deliver each node's message ([`super::deliver`]), wait for the Stop hook,
//! evaluate, and keep the process alive afterwards until it exits or is
//! killed.
//!
//! Start: the `start` hook line, or (older CLIs without SessionStart) 3 s of
//! quiet after the first output. A blocking dialog (workspace trust, a new MCP
//! server) holds everything: the start timeout and the quiet fallback pause,
//! a `notification` event asks the user to answer in the window, and nothing
//! is typed until the screen shows Claude Code's empty input box again.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, watch};

use crate::terminal::api::PromptAction;
use crate::terminal::hooks::HookLine;
use crate::terminal::transcript::Transcript;

use crate::contract::{NodeEvent, NodeEventKind, NodeStatus, SessionStatus, TerminalSession};
use crate::harness::adapter::{behavior, Behavior};
use crate::harness::claude;
use crate::store::write_json;
use crate::terminal::deliver::{asking, dialog_due};
use crate::terminal::hooks::{Hook, HookLog, HOOKS_LOG};
use crate::terminal::node::{record, Finished};
use crate::terminal::port::{tick, Tick};
use crate::terminal::pty::Pty;
use crate::terminal::screen::plain_tail;
use crate::terminal::{
    session_dir, SessionFile, SessionPlan, SessionShared, TermMsg, SESSION_FILE,
};
use crate::turns::RunCtx;

pub(super) const POLL: Duration = Duration::from_millis(300);
const TAIL_CHARS: usize = 300;
pub(super) const CANCELLED: &str = "cancelled by user";

/// A node's end: status, reason, and the session's end if it ended too.
pub(super) type NodeEnd = (NodeStatus, Option<String>, Option<End>);

/// Per-node counts for summary.json.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Stats {
    pub questions: usize,
    pub waited: bool,
}

/// Why a session stopped before its node list was done.
pub(super) enum End {
    Cancelled,
    Exited(u32),
    Failed(String),
}

/// Everything a session task needs.
pub struct SessionTask {
    pub ctx: Arc<RunCtx>,
    pub plan: SessionPlan,
    pub shared: Arc<SessionShared>,
    pub tx: mpsc::UnboundedSender<TermMsg>,
}

pub(super) struct Session {
    pub(super) t: SessionTask,
    pub(super) kill: watch::Receiver<bool>,
    pub(super) pty: Option<Arc<Pty>>,
    pub(super) exit: Option<watch::Receiver<Option<u32>>>,
    pub(super) hooks: HookLog,
    pub(super) started_at: DateTime<Utc>,
    pub(super) finished_at: Option<DateTime<Utc>>,
    pub(super) done: BTreeMap<String, NodeStatus>,
    /// Output mark when the dialog on screen was announced.
    pub(super) announced: Option<u64>,
    /// Transcript span of the node turn that just ended (see `send.rs`).
    pub(super) span: Option<(u64, u64)>,
    /// How the session's harness is read (markers, submit, stop).
    pub(super) behavior: Behavior,
    /// Claude Code's transcript JSONL (path from the hook payloads).
    pub(super) transcript: Option<Transcript>,
    /// Answers and "continue" from the app.
    pub(super) actions: Option<mpsc::UnboundedReceiver<PromptAction>>,
    /// The current node's questions.
    pub(super) stats: Stats,
    /// Prompt ids handed out in this session.
    pub(super) prompt_seq: u32,
    /// A chained-headless session's program and CLI session id.
    pub(super) chain: Option<crate::terminal::chained::Chain>,
}

impl SessionTask {
    /// Run the session to its end (process exited or killed).
    pub async fn run(self) {
        let dir = session_dir(&self.ctx.graph_dir, &self.ctx.run_id, &self.plan.sid);
        let behavior = self
            .ctx
            .harness(&self.plan.harness)
            .map(behavior)
            .unwrap_or_else(|_| claude::behavior());
        self.shared
            .set_delivery(behavior.expect.clone(), behavior.ready_quiet);
        let actions = self.shared.take_actions();
        let mut s = Session {
            behavior,
            kill: self.shared.kill_watch(),
            hooks: HookLog::new(dir.join(HOOKS_LOG)),
            t: self,
            pty: None,
            exit: None,
            started_at: Utc::now(),
            finished_at: None,
            done: BTreeMap::new(),
            announced: None,
            span: None,
            transcript: None,
            actions,
            stats: Stats::default(),
            prompt_seq: 0,
            chain: None,
        };
        s.drive().await;
        if let Some(pty) = &s.pty {
            if pty.exit_code().is_none() {
                pty.kill().await;
            }
        }
        s.report(|i| i.pid = None);
        s.t.shared.task_ended();
    }
}

impl Session {
    async fn drive(&mut self) {
        let end = match self.start().await {
            Ok(()) => {
                self.report(|i| i.status = SessionStatus::Running);
                self.send_nodes().await
            }
            Err(end) => {
                let nodes = self.t.plan.to_send().to_vec();
                self.close_nodes(&nodes, &end);
                Some(end)
            }
        };
        self.finished_at = Some(Utc::now());
        let status = match end {
            None => SessionStatus::Done,
            Some(End::Cancelled) => SessionStatus::Cancelled,
            Some(End::Exited(_) | End::Failed(_)) => SessionStatus::Failed,
        };
        if let Some(End::Failed(reason)) = &end {
            self.event(NodeEventKind::Stderr, reason.clone());
        }
        self.report(|i| {
            i.status = status;
            i.current_node = None;
        });
        if end.is_none() {
            if self.chain.is_some() {
                // Done: follow-up messages run until the session is killed.
                self.serve_followups().await;
            } else {
                // Done: the user keeps talking in the window until it exits or is killed.
                while let Tick::Poll = self.tick(Duration::from_secs(3600)).await {}
            }
        }
    }

    /// Publish a change of the public state (session.json, run loop).
    pub(super) fn report(&self, f: impl FnOnce(&mut TerminalSession)) {
        let info = self.t.shared.update(f);
        let file = SessionFile {
            sid: info.sid.clone(),
            title: info.title.clone(),
            node_ids: info.node_ids.clone(),
            pid: info.pid,
            status: info.status,
            harness: info.harness.clone(),
            started_at: self.started_at,
            finished_at: self.finished_at,
        };
        let dir = session_dir(&self.t.ctx.graph_dir, &self.t.ctx.run_id, &info.sid);
        if let Err(e) = write_json(&dir.join(SESSION_FILE), &file) {
            log::warn!("writing {SESSION_FILE} of {} failed: {e}", info.sid);
        }
        let _ = self.t.tx.send(TermMsg::Session(info));
    }

    /// Report a node's status; `start`: it starts now (else it ends now).
    pub(super) fn node_msg(
        &self,
        node_id: &str,
        status: NodeStatus,
        reason: Option<String>,
        start: bool,
    ) {
        let now = Some(Utc::now());
        let msg = TermMsg::Node {
            node_id: node_id.to_string(),
            status,
            reason,
            started_at: if start { now } else { None },
            finished_at: if start { None } else { now },
        };
        let _ = self.t.tx.send(msg);
    }

    /// A node event on the current (else first) node of the session.
    pub(super) fn event(&self, kind: NodeEventKind, text: String) {
        let info = self.t.shared.info();
        let node_id = info
            .current_node
            .or_else(|| self.t.plan.to_send().first().cloned())
            .unwrap_or_default();
        self.t.ctx.emitter.node_event(&NodeEvent {
            run_id: self.t.ctx.run_id.clone(),
            node_id,
            kind,
            text,
            ts: Utc::now(),
        });
    }

    pub(super) async fn tick(&mut self, wait: Duration) -> Tick {
        tick(&mut self.kill, self.exit.clone(), wait).await
    }

    pub(super) fn poll_hooks(&mut self) -> Vec<Hook> {
        let lines = self.hooks.poll_lines().unwrap_or_else(|e| {
            log::warn!("reading {HOOKS_LOG} failed: {e}");
            Vec::new()
        });
        lines.iter().for_each(|l| self.note_transcript(l));
        lines.into_iter().map(|l| l.hook).collect()
    }

    /// Remember the transcript path a hook payload names (it changes after
    /// `/clear`).
    pub(super) fn note_transcript(&mut self, line: &HookLine) {
        let Some(path) = line.field("transcript_path") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        if self.transcript.as_ref().map(|t| t.path()) != Some(path.as_path()) {
            self.transcript = Transcript::new(path);
        }
    }

    pub(super) fn tail(&self) -> String {
        let text = self
            .pty
            .as_ref()
            .map(|p| p.output.replay().text)
            .unwrap_or_default();
        plain_tail(&text, TAIL_CHARS)
    }

    /// True while a dialog is on screen; announces each dialog once.
    pub(super) fn dialog_open(&mut self) -> bool {
        let Some(pty) = self.pty.clone() else {
            return false;
        };
        let out = &pty.output;
        let screen = out.screen();
        let quiet = out.last_output().map_or(Duration::ZERO, |t| t.elapsed());
        if dialog_due(&screen, quiet, self.announced, |m| out.screen_since(m)) {
            self.event(NodeEventKind::Notification, asking(&self.t.plan.title));
            self.announced = Some(out.mark());
        }
        if !screen.dialog_open() {
            self.announced = None;
        }
        screen.dialog_open()
    }

    pub(super) fn quiet_for(&self, d: Duration) -> bool {
        self.pty
            .as_ref()
            .and_then(|p| p.output.last_output())
            .is_some_and(|t| t.elapsed() >= d)
    }

    /// Mark `nodes` (not started) as ended by `end`.
    fn close_nodes(&self, nodes: &[String], end: &End) {
        let (status, reason) = match end {
            End::Cancelled => (NodeStatus::Cancelled, CANCELLED.to_string()),
            End::Exited(code) => (
                NodeStatus::Skipped,
                format!(
                    "{} exited (code {code}) before this node",
                    self.behavior.label
                ),
            ),
            End::Failed(r) => (NodeStatus::Failed, r.clone()),
        };
        for id in nodes {
            self.node_msg(id, status, Some(reason.clone()), false);
        }
    }

    async fn send_nodes(&mut self) -> Option<End> {
        let nodes = self.t.plan.to_send().to_vec();
        for (i, id) in nodes.iter().enumerate() {
            let started_at = Utc::now();
            self.stats = Stats::default();
            let (status, reason, end) = self.run_node(id, started_at).await;
            let finished = Finished {
                questions: self.stats.questions,
                waited_for_user: self.stats.waited,
                sid: &self.t.plan.sid,
                node_id: id,
                status,
                reason: reason.clone(),
                started_at,
                finished_at: Utc::now(),
                transcript: self.span.take(),
            };
            if let Err(e) = record(&self.t.ctx, &finished) {
                log::error!("recording node {id} failed: {e}");
            }
            self.done.insert(id.clone(), status);
            self.node_msg(id, status, reason, false);
            let rest = &nodes[i + 1..];
            if let Some(end) = end {
                self.close_nodes(rest, &end);
                return Some(end);
            }
            if status.is_failure() && self.t.ctx.graph.settings.stop_on_failure {
                for r in rest {
                    self.node_msg(
                        r,
                        NodeStatus::Skipped,
                        Some(format!("stopped after {id}")),
                        false,
                    );
                }
                return None;
            }
        }
        None
    }
}

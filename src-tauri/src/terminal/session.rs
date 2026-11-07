//! One terminal session: spawn Claude Code in a pty, wait for it to start,
//! type each node's message, wait for the Stop hook, evaluate, and keep the
//! process alive afterwards until it exits or is killed.
//!
//! Start: the `start` hook line, or (older CLIs without SessionStart) 3 s of
//! quiet after the first output. A blocking dialog (workspace trust, a new MCP
//! server) holds everything: the start timeout and the quiet fallback pause,
//! a `notification` event asks the user to answer in the window, and the
//! first message is typed only once the dialog was answered and the screen
//! has been quiet for a moment.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, watch};

use crate::contract::{
    Model, NodeEvent, NodeEventKind, NodeStatus, SessionStatus, TerminalSession,
};
use crate::runner::model_flag;
use crate::store::write_json;
use crate::terminal::hooks::{Hook, HookLog, HOOKS_LOG};
use crate::terminal::message::Typed;
use crate::terminal::node::{evaluate, prepare, record, Finished};
use crate::terminal::pty::Pty;
use crate::terminal::screen::plain_tail;
use crate::terminal::{
    session_dir, SessionFile, SessionPlan, SessionShared, TermMsg, SESSION_FILE,
};
use crate::turns::RunCtx;

pub(super) const POLL: Duration = Duration::from_millis(300);
const MODEL_WAIT: Duration = Duration::from_millis(700);
const PASTE_WAIT: Duration = Duration::from_millis(300);
const TAIL_CHARS: usize = 300;
const CANCELLED: &str = "cancelled by user";

pub(super) enum Tick {
    Poll,
    Killed,
    Exited(u32),
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
    pub(super) waiting_noted: bool,
}

impl SessionTask {
    /// Run the session to its end (process exited or killed).
    pub async fn run(self) {
        let dir = session_dir(&self.ctx.graph_dir, &self.ctx.run_id, &self.plan.sid);
        let mut s = Session {
            kill: self.shared.kill_watch(),
            hooks: HookLog::new(dir.join(HOOKS_LOG)),
            t: self,
            pty: None,
            exit: None,
            started_at: Utc::now(),
            finished_at: None,
            done: BTreeMap::new(),
            waiting_noted: false,
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
            // Done: the user keeps talking in the window until it exits or is killed.
            while let Tick::Poll = self.tick(Duration::from_secs(3600)).await {}
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
            started_at: self.started_at,
            finished_at: self.finished_at,
        };
        let dir = session_dir(&self.t.ctx.graph_dir, &self.t.ctx.run_id, &info.sid);
        if let Err(e) = write_json(&dir.join(SESSION_FILE), &file) {
            log::warn!("writing {SESSION_FILE} of {} failed: {e}", info.sid);
        }
        let _ = self.t.tx.send(TermMsg::Session(info));
    }

    fn node_msg(&self, node_id: &str, status: NodeStatus, reason: Option<String>, start: bool) {
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
    fn event(&self, kind: NodeEventKind, text: String) {
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
        let exit = self.exit.clone();
        let exited = async move {
            match exit {
                Some(mut rx) => {
                    let code = rx.wait_for(Option::is_some).await.map(|c| *c);
                    match code {
                        Ok(code) => code.unwrap_or(u32::MAX),
                        Err(_) => std::future::pending().await,
                    }
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => Tick::Poll,
            Ok(_) = self.kill.wait_for(|k| *k) => Tick::Killed,
            code = exited => Tick::Exited(code),
        }
    }

    pub(super) fn poll_hooks(&mut self) -> Vec<Hook> {
        self.hooks.poll().unwrap_or_else(|e| {
            log::warn!("reading {HOOKS_LOG} failed: {e}");
            Vec::new()
        })
    }

    pub(super) fn tail(&self) -> String {
        let text = self
            .pty
            .as_ref()
            .map(|p| p.output.replay().text)
            .unwrap_or_default();
        plain_tail(&text, TAIL_CHARS)
    }

    /// True while a dialog waits for the user; notes it once per dialog.
    pub(super) fn dialog_open(&mut self) -> bool {
        let open = self.pty.as_ref().is_some_and(|p| p.output.dialog_open());
        if open && !self.waiting_noted {
            let title = self.t.plan.title.clone();
            self.event(
                NodeEventKind::Notification,
                format!("Claude Code is waiting for you in window {title}"),
            );
        }
        self.waiting_noted = open;
        open
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
                format!("Claude Code exited (code {code}) before this node"),
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
            let (status, reason, end) = self.run_node(id, started_at).await;
            let finished = Finished {
                sid: &self.t.plan.sid,
                node_id: id,
                status,
                reason: reason.clone(),
                started_at,
                finished_at: Utc::now(),
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

    fn write(&self, bytes: &[u8]) -> bool {
        self.pty
            .as_ref()
            .is_some_and(|p| p.write(bytes, false).is_ok())
    }

    async fn run_node(
        &mut self,
        id: &str,
        started_at: DateTime<Utc>,
    ) -> (NodeStatus, Option<String>, Option<End>) {
        let started = SystemTime::from(started_at);
        self.node_msg(id, NodeStatus::Running, None, true);
        self.report(|i| i.current_node = Some(id.to_string()));
        let ctx = self.t.ctx.clone();
        let Ok(node) = ctx.node(id) else {
            return (NodeStatus::Failed, Some(format!("no node {id}")), None);
        };
        let typed = match prepare(&ctx, node, &self.done) {
            Ok(t) => t,
            Err(e) => {
                let reason = format!("could not prepare the node: {}", e.message);
                return (NodeStatus::Failed, Some(reason), None);
            }
        };
        self.poll_hooks(); // stops of earlier, user-typed prompts do not count
        let mut sent = true;
        if let Some(m) = model_flag(node.model, Model::Inherit) {
            sent &= self.write(format!("/model {m}\r").as_bytes());
            if let Some(end) = self.pause(MODEL_WAIT).await {
                return ended(end);
            }
        }
        match &typed {
            Typed::Line(_) => sent &= self.write(format!("{}\r", typed.payload()).as_bytes()),
            Typed::Paste(_) => {
                sent &= self.write(typed.payload().as_bytes());
                if let Some(end) = self.pause(PASTE_WAIT).await {
                    return ended(end);
                }
                sent &= self.write(b"\r");
            }
        }
        if !sent {
            return ended(End::Exited(
                self.pty
                    .as_ref()
                    .and_then(|p| p.exit_code())
                    .unwrap_or(u32::MAX),
            ));
        }
        let title = self.t.plan.title.clone();
        self.event(
            NodeEventKind::Text,
            format!("sent node {id} to window {title}"),
        );
        loop {
            match self.tick(POLL).await {
                Tick::Killed => return ended(End::Cancelled),
                Tick::Exited(code) => return ended(End::Exited(code)),
                Tick::Poll => {}
            }
            let hooks = self.poll_hooks();
            if hooks.contains(&Hook::Notify) {
                self.event(
                    NodeEventKind::Notification,
                    format!("Claude Code is waiting for you in window {title}"),
                );
            }
            if hooks.contains(&Hook::Stop) {
                let (status, reason) = evaluate(&ctx, node, started);
                return (status, reason, None);
            }
        }
    }

    /// Sleep `d` unless the session is killed or exits first.
    async fn pause(&mut self, d: Duration) -> Option<End> {
        match self.tick(d).await {
            Tick::Poll => None,
            Tick::Killed => Some(End::Cancelled),
            Tick::Exited(code) => Some(End::Exited(code)),
        }
    }
}

/// The node's own status when the session ends while it runs.
fn ended(end: End) -> (NodeStatus, Option<String>, Option<End>) {
    let (status, reason) = match &end {
        End::Cancelled => (NodeStatus::Cancelled, CANCELLED.to_string()),
        End::Exited(code) => (
            NodeStatus::Failed,
            format!("Claude Code exited (code {code}) before the node finished"),
        ),
        End::Failed(r) => (NodeStatus::Failed, r.clone()),
    };
    (status, Some(reason), Some(end))
}

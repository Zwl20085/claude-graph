//! The terminal-mode run loop: starts up to `max_parallel` sessions at once,
//! applies their reports to the run state, handles cancel and follow-up
//! messages, and finishes the run when every session is settled (done,
//! failed or cancelled). Sessions outlive the loop: their processes stay alive
//! for the user until they exit, are cancelled, or the app exits.
//!
//! Run status: cancelled after `cancel_run`; else failed if any node is
//! failed, blocked, skipped or cancelled (nodes skipped because they come
//! before the start node of a `from` / `failed_of` run do not count); else
//! succeeded.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;

use crate::contract::{
    NodeRunState, NodeStatus, RunMode, RunState, RunStatus, SessionStatus, TerminalSession,
};
use crate::run_loop::{blocking, LoopMsg};
use crate::run_store::{append_log, write_run_state};
use crate::terminal::components::BEFORE_START;
use crate::terminal::session::SessionTask;
use crate::terminal::{SessionPlan, SessionShared, TermMsg, Terminals};
use crate::turns::RunCtx;

const FINAL_WRITE_ATTEMPTS: u32 = 3;
const CANCELLED: &str = "cancelled by user";

/// Fresh run state: nodes before a start node skipped, the others queued.
pub fn initial_state(ctx: &RunCtx, plans: &[SessionPlan]) -> RunState {
    let node = |status, reason: Option<&str>| NodeRunState {
        status,
        reason: reason.map(str::to_string),
        ..NodeRunState::pending()
    };
    let nodes = plans
        .iter()
        .flat_map(|p| {
            let before = p
                .before_start()
                .iter()
                .map(|id| (id.clone(), node(NodeStatus::Skipped, Some(BEFORE_START))));
            let sent = p
                .to_send()
                .iter()
                .map(|id| (id.clone(), node(NodeStatus::Queued, None)));
            before.chain(sent).collect::<Vec<_>>()
        })
        .collect();
    RunState {
        run_id: ctx.run_id.clone(),
        graph_dir: ctx.graph_dir.display().to_string(),
        started_at: Utc::now(),
        finished_at: None,
        status: RunStatus::Running,
        nodes,
        mode: RunMode::Terminal,
        sessions: plans.iter().map(session_info).collect(),
    }
}

fn session_info(p: &SessionPlan) -> TerminalSession {
    TerminalSession {
        sid: p.sid.clone(),
        title: p.title.clone(),
        node_ids: p.node_ids.clone(),
        status: SessionStatus::Starting,
        pid: None,
        current_node: None,
    }
}

pub struct TerminalLoop {
    ctx: Arc<RunCtx>,
    plans: Vec<SessionPlan>,
    state: RunState,
    terminals: Terminals,
    shared: Vec<Arc<SessionShared>>,
    tx: mpsc::UnboundedSender<LoopMsg>,
    rx: mpsc::UnboundedReceiver<LoopMsg>,
    term_tx: mpsc::UnboundedSender<TermMsg>,
    term_rx: mpsc::UnboundedReceiver<TermMsg>,
    /// Sids whose task was started, or that were cancelled before starting.
    launched: BTreeSet<String>,
    cancelled: bool,
    persisted: Option<RunState>,
}

impl TerminalLoop {
    /// Registers every session of `plans` in `terminals` (not started yet).
    pub fn new(ctx: Arc<RunCtx>, plans: Vec<SessionPlan>, terminals: Terminals) -> Self {
        let state = initial_state(&ctx, &plans);
        let shared: Vec<Arc<SessionShared>> = plans
            .iter()
            .map(|p| SessionShared::new(&ctx.run_id, &ctx.graph_dir, session_info(p)))
            .collect();
        shared.iter().for_each(|s| terminals.insert(s.clone()));
        let (tx, rx) = mpsc::unbounded_channel();
        let (term_tx, term_rx) = mpsc::unbounded_channel();
        Self {
            ctx,
            plans,
            state,
            terminals,
            shared,
            tx,
            rx,
            term_tx,
            term_rx,
            launched: BTreeSet::new(),
            cancelled: false,
            persisted: None,
        }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<LoopMsg> {
        self.tx.clone()
    }

    pub async fn run(mut self) -> RunState {
        loop {
            self.launch();
            self.publish().await;
            if self.all_settled() {
                break;
            }
            tokio::select! {
                Some(m) = self.term_rx.recv() => self.apply(m),
                Some(m) = self.rx.recv() => self.handle(m),
            }
        }
        self.finish().await
    }

    fn all_settled(&self) -> bool {
        self.state.sessions.iter().all(|s| s.status.is_settled())
    }

    fn active(&self) -> usize {
        self.state
            .sessions
            .iter()
            .filter(|s| self.launched.contains(&s.sid) && !s.status.is_settled())
            .count()
    }

    /// Start waiting sessions while fewer than `max_parallel` are active.
    fn launch(&mut self) {
        let max = self.ctx.graph.settings.max_parallel.max(1) as usize;
        for i in 0..self.plans.len() {
            if self.cancelled || self.active() >= max {
                return;
            }
            let sid = self.plans[i].sid.clone();
            if !self.launched.insert(sid) {
                continue;
            }
            let task = SessionTask {
                ctx: self.ctx.clone(),
                plan: self.plans[i].clone(),
                shared: self.shared[i].clone(),
                tx: self.term_tx.clone(),
            };
            self.shared[i].task_started();
            tokio::spawn(task.run());
        }
    }

    fn handle(&mut self, msg: LoopMsg) {
        match msg {
            LoopMsg::CancelRun => {
                self.cancelled = true;
                self.terminals.kill_run(&self.ctx.run_id);
                (0..self.plans.len()).for_each(|i| self.cancel_waiting(i));
            }
            LoopMsg::CancelNode(id) => {
                if let Some(i) = self.plans.iter().position(|p| p.node_ids.contains(&id)) {
                    self.shared[i].request_kill();
                    self.cancel_waiting(i);
                }
            }
            LoopMsg::SendMessage {
                node_id,
                text,
                reply,
            } => {
                let result = self.terminals.type_message(
                    &self.ctx.run_id,
                    &node_id,
                    &text,
                    self.ctx.emitter.clone(),
                );
                if reply.send(result).is_err() {
                    log::warn!("message to node {node_id}: the caller went away");
                }
            }
            LoopMsg::TurnDone { node_id, .. } => {
                log::warn!("terminal run got a headless turn report for {node_id}");
            }
        }
    }

    /// A session that has not started yet is cancelled with its nodes.
    fn cancel_waiting(&mut self, i: usize) {
        let sid = self.plans[i].sid.clone();
        if !self.launched.insert(sid.clone()) {
            return;
        }
        let now = Some(Utc::now());
        for id in self.plans[i].to_send() {
            if let Some(n) = self.state.nodes.get_mut(id) {
                n.status = NodeStatus::Cancelled;
                n.reason = Some(CANCELLED.to_string());
                n.finished_at = now;
            }
        }
        let info = self.shared[i].update(|s| s.status = SessionStatus::Cancelled);
        self.apply(TermMsg::Session(info));
    }

    fn apply(&mut self, msg: TermMsg) {
        match msg {
            TermMsg::Node {
                node_id,
                status,
                reason,
                started_at,
                finished_at,
            } => {
                if let Some(n) = self.state.nodes.get_mut(&node_id) {
                    n.status = status;
                    n.reason = reason;
                    n.started_at = started_at.or(n.started_at);
                    n.finished_at = finished_at.or(n.finished_at);
                }
            }
            TermMsg::Session(info) => {
                if let Some(s) = self.state.sessions.iter_mut().find(|s| s.sid == info.sid) {
                    *s = info;
                }
            }
        }
    }

    async fn write_state(&self) -> crate::contract::AppResult<()> {
        let (dir, state) = (self.ctx.graph_dir.clone(), self.state.clone());
        blocking(move || write_run_state(&dir, &state)).await
    }

    async fn publish(&mut self) {
        if self.persisted.as_ref() == Some(&self.state) {
            return;
        }
        match self.write_state().await {
            Ok(()) => self.persisted = Some(self.state.clone()),
            Err(e) => log::error!("writing run.json failed: {e}"),
        }
        self.ctx.emitter.run_state(&self.state);
    }

    fn succeeded(&self) -> bool {
        self.state.nodes.values().all(|n| match n.status {
            NodeStatus::Skipped => n.reason.as_deref() == Some(BEFORE_START),
            NodeStatus::Succeeded => true,
            NodeStatus::Pending
            | NodeStatus::Queued
            | NodeStatus::Running
            | NodeStatus::Failed
            | NodeStatus::Blocked
            | NodeStatus::Cancelled => false,
        })
    }

    async fn finish(mut self) -> RunState {
        // Nodes cancelled before their session started get their log line here.
        for (id, n) in &self.state.nodes {
            if n.started_at.is_none() && n.status == NodeStatus::Cancelled {
                let (dir, run_id, id) = (
                    self.ctx.graph_dir.clone(),
                    self.ctx.run_id.clone(),
                    id.clone(),
                );
                let logged =
                    blocking(move || append_log(&dir, &run_id, &id, "cancelled", None, None)).await;
                if let Err(e) = logged {
                    log::warn!("GRAPH_LOG append failed: {e}");
                }
            }
        }
        self.state.status = if self.cancelled {
            RunStatus::Cancelled
        } else if self.succeeded() {
            RunStatus::Succeeded
        } else {
            RunStatus::Failed
        };
        self.state.finished_at = Some(Utc::now());
        for attempt in 1..=FINAL_WRITE_ATTEMPTS {
            match self.write_state().await {
                Ok(()) => break,
                Err(e) => {
                    log::error!("writing the final run.json (attempt {attempt}) failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
                }
            }
        }
        self.ctx.emitter.run_state(&self.state);
        self.state
    }
}

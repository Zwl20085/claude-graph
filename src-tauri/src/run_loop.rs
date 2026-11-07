//! The run loop: drives the pure [`Scheduler`] with tokio tasks.
//!
//! All mutation happens on the loop task. Node turns run as spawned tasks and
//! report back through the same channel that carries cancel and follow-up
//! messages, so ordering is decided in one place. After every message the loop
//! rewrites run.json and emits `run:state` if anything changed.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{mpsc, oneshot, watch};

use crate::contract::{
    AppError, AppResult, MessageRole, NodeEvent, NodeEventKind, NodeRunState, NodeStatus, RunState,
    RunStatus,
};
use crate::run_store::{append_log, write_run_state};
use crate::runner::{run_turn, TurnOutcome, TurnSpec};
use crate::scheduler::{CancelEffect, Scheduler};
use crate::turns::{
    followup_turn, initial_turn, record_followup, record_initial, save_message, RunCtx,
};

pub enum LoopMsg {
    TurnDone {
        node_id: String,
        followup: bool,
        outcome: TurnOutcome,
    },
    CancelRun,
    CancelNode(String),
    SendMessage {
        node_id: String,
        text: String,
        reply: oneshot::Sender<AppResult<()>>,
    },
}

pub struct RunLoop {
    ctx: Arc<RunCtx>,
    sched: Scheduler,
    state: RunState,
    /// Nodes already terminal when the loop started (resumed runs).
    initially_terminal: BTreeSet<String>,
    kills: HashMap<String, watch::Sender<bool>>,
    queues: HashMap<String, VecDeque<String>>,
    tx: mpsc::UnboundedSender<LoopMsg>,
    rx: mpsc::UnboundedReceiver<LoopMsg>,
    persisted: Option<RunState>,
}

fn is_terminal(s: NodeStatus) -> bool {
    matches!(
        s,
        NodeStatus::Succeeded | NodeStatus::Failed | NodeStatus::Skipped | NodeStatus::Cancelled
    )
}

/// Fresh `RunState` for a new run of `sched`'s target.
pub fn initial_state(ctx: &RunCtx, sched: &Scheduler) -> RunState {
    RunState {
        run_id: ctx.run_id.clone(),
        graph_dir: ctx.graph_dir.display().to_string(),
        started_at: Utc::now(),
        finished_at: None,
        status: RunStatus::Running,
        nodes: sched
            .target()
            .iter()
            .map(|id| {
                let node = NodeRunState {
                    status: sched.status(id).unwrap_or(NodeStatus::Pending),
                    reason: sched.reason(id).map(str::to_string),
                    ..NodeRunState::pending()
                };
                (id.clone(), node)
            })
            .collect(),
    }
}

impl RunLoop {
    pub fn new(ctx: Arc<RunCtx>, sched: Scheduler, state: RunState) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let initially_terminal = state
            .nodes
            .iter()
            .filter(|(_, s)| is_terminal(s.status))
            .map(|(id, _)| id.clone())
            .collect();
        Self {
            ctx,
            sched,
            state,
            initially_terminal,
            kills: HashMap::new(),
            queues: HashMap::new(),
            tx,
            rx,
            persisted: None,
        }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<LoopMsg> {
        self.tx.clone()
    }

    pub async fn run(mut self) -> RunState {
        self.state.status = RunStatus::Running;
        self.state.finished_at = None;
        loop {
            self.start_ready();
            self.sync_and_publish();
            if self.sched.is_done() {
                // Messages already in the channel (a follow-up request) still count.
                match self.rx.try_recv() {
                    Ok(msg) => {
                        self.handle(msg);
                        continue;
                    }
                    Err(_) => break,
                }
            }
            match self.rx.recv().await {
                Some(msg) => self.handle(msg),
                None => break,
            }
        }
        self.finish()
    }

    fn handle(&mut self, msg: LoopMsg) {
        match msg {
            LoopMsg::TurnDone {
                node_id,
                followup,
                outcome,
            } => self.on_turn_done(&node_id, followup, outcome),
            LoopMsg::CancelRun => {
                for id in self.sched.cancel_all() {
                    self.kill(&id);
                }
                self.queues.clear();
            }
            LoopMsg::CancelNode(id) => {
                if self.sched.cancel_node(&id) == CancelEffect::MustKill {
                    self.kill(&id);
                }
                self.queues.remove(&id);
            }
            LoopMsg::SendMessage {
                node_id,
                text,
                reply,
            } => {
                let result = self.on_message(&node_id, text);
                let _ = reply.send(result);
            }
        }
    }

    fn kill(&self, id: &str) {
        if let Some(k) = self.kills.get(id) {
            let _ = k.send(true);
        }
    }

    fn start_ready(&mut self) {
        for id in self.sched.ready() {
            let now = Utc::now();
            if let Some(node) = self.state.nodes.get_mut(&id) {
                node.started_at = Some(now);
                node.status = NodeStatus::Running;
            }
            self.sync_and_publish();
            match initial_turn(&self.ctx, &self.state, &self.sched, &id, now) {
                Ok(spec) => self.spawn_turn(spec, false),
                Err(e) => {
                    let reason = format!("could not prepare the node: {}", e.message);
                    self.sched
                        .on_finished(&id, NodeStatus::Failed, Some(reason));
                }
            }
        }
    }

    fn spawn_turn(&mut self, spec: TurnSpec, followup: bool) {
        let (kill_tx, kill_rx) = watch::channel(false);
        self.kills.insert(spec.node_id.clone(), kill_tx);
        let tx = self.tx.clone();
        let emitter = self.ctx.emitter.clone();
        tokio::spawn(async move {
            let node_id = spec.node_id.clone();
            let outcome = run_turn(spec, emitter, kill_rx).await;
            let _ = tx.send(LoopMsg::TurnDone {
                node_id,
                followup,
                outcome,
            });
        });
    }

    fn emit_event(&self, node_id: &str, kind: NodeEventKind, text: String) {
        self.ctx.emitter.node_event(&NodeEvent {
            run_id: self.ctx.run_id.clone(),
            node_id: node_id.to_string(),
            kind,
            text,
            ts: Utc::now(),
        });
    }

    fn on_turn_done(&mut self, id: &str, followup: bool, outcome: TurnOutcome) {
        self.kills.remove(id);
        let now = Utc::now();
        if followup {
            self.sched.release(id);
        }
        let recorded = if followup {
            record_followup(&self.ctx, id, &outcome, now).map(|s| (s.cost_usd, s.num_turns))
        } else {
            record_initial(&self.ctx, id, &outcome).map(|_| (outcome.cost_usd, outcome.num_turns))
        };
        if let Err(e) = &recorded {
            log::error!("recording node {id} of run {} failed: {e}", self.ctx.run_id);
        }
        if let Some(node) = self.state.nodes.get_mut(id) {
            node.session_id = outcome.session_id.clone().or(node.session_id.take());
            if let Ok((cost, turns)) = recorded {
                node.cost_usd = cost;
                node.num_turns = turns;
            }
            node.finished_at = Some(now);
        }
        let log_status = if followup {
            "followup".to_string()
        } else {
            status_text(outcome.status)
        };
        if let Err(e) = append_log(
            &self.ctx.graph_dir,
            &self.ctx.run_id,
            id,
            &log_status,
            outcome.cost_usd,
            Some(outcome.duration_ms),
        ) {
            log::error!("GRAPH_LOG append failed: {e}");
        }
        self.next_followup(id, &outcome);
        self.sched
            .on_finished(id, outcome.status, outcome.reason.clone());
    }

    /// Start the next queued follow-up of `id`, or drop the queue if impossible.
    fn next_followup(&mut self, id: &str, outcome: &TurnOutcome) {
        let session = self.state.nodes.get(id).and_then(|n| n.session_id.clone());
        let Some(queue) = self.queues.get_mut(id) else {
            return;
        };
        let Some(text) = queue.pop_front() else {
            return;
        };
        let spec = match (outcome.status, session) {
            (NodeStatus::Cancelled, _) => None,
            (_, Some(sid)) => followup_turn(&self.ctx, id, &sid, &text).ok(),
            (_, None) => None,
        };
        match spec {
            Some(spec) => self.spawn_turn(spec, true),
            None => {
                let dropped = 1 + queue.len();
                queue.clear();
                for _ in 0..dropped {
                    self.sched.release(id);
                }
                self.emit_event(
                    id,
                    NodeEventKind::Stderr,
                    format!(
                        "{dropped} queued message(s) dropped: the node has no session to resume"
                    ),
                );
            }
        }
    }

    fn on_message(&mut self, id: &str, text: String) -> AppResult<()> {
        let status = self
            .sched
            .status(id)
            .ok_or_else(|| AppError::new("not_found", format!("node {id} is not in this run")))?;
        let session = self.state.nodes.get(id).and_then(|n| n.session_id.clone());
        match status {
            NodeStatus::Running => {
                self.sched.hold(id);
                self.queues
                    .entry(id.to_string())
                    .or_default()
                    .push_back(text.clone());
                save_message(&self.ctx, id, MessageRole::User, &text)?;
                self.emit_event(id, NodeEventKind::UserMessage, text.clone());
                Ok(())
            }
            NodeStatus::Succeeded | NodeStatus::Failed | NodeStatus::Cancelled
                if session.is_some() =>
            {
                let sid = session.unwrap_or_default();
                let spec = followup_turn(&self.ctx, id, &sid, &text)?;
                self.sched.reopen(id);
                if let Some(node) = self.state.nodes.get_mut(id) {
                    node.status = NodeStatus::Running;
                    node.reason = None;
                }
                save_message(&self.ctx, id, MessageRole::User, &text)?;
                self.emit_event(id, NodeEventKind::UserMessage, text);
                self.spawn_turn(spec, true);
                Ok(())
            }
            _ => Err(AppError::new("not_found", "node has not run yet")),
        }
    }

    /// Copy scheduler statuses into the run state; persist and emit on change.
    fn sync_and_publish(&mut self) {
        for (id, node) in self.state.nodes.iter_mut() {
            if let Some(status) = self.sched.status(id) {
                if status != node.status {
                    node.status = status;
                    node.reason = self.sched.reason(id).map(str::to_string);
                }
            }
        }
        if self.persisted.as_ref() == Some(&self.state) {
            return;
        }
        if let Err(e) = write_run_state(&self.ctx.graph_dir, &self.state) {
            log::error!("writing run.json failed: {e}");
        }
        self.ctx.emitter.run_state(&self.state);
        self.persisted = Some(self.state.clone());
    }

    fn finish(mut self) -> RunState {
        self.sched.finalize();
        self.sync_and_publish();
        for (id, node) in &self.state.nodes {
            let never_ran = node.started_at.is_none() && !self.initially_terminal.contains(id);
            if never_ran && is_terminal(node.status) {
                let _ = append_log(
                    &self.ctx.graph_dir,
                    &self.ctx.run_id,
                    id,
                    &status_text(node.status),
                    None,
                    None,
                );
            }
        }
        self.state.status = if self.sched.is_cancelled() {
            RunStatus::Cancelled
        } else if self.sched.run_succeeded() {
            RunStatus::Succeeded
        } else {
            RunStatus::Failed
        };
        self.state.finished_at = Some(Utc::now());
        self.sync_and_publish();
        self.state
    }
}

fn status_text(status: NodeStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

//! The run loop: drives the pure [`Scheduler`] with tokio tasks.
//!
//! All mutation happens on the loop task. Node turns run as spawned tasks and
//! report back through the same channel that carries cancel and follow-up
//! messages, so ordering is decided in one place. After every message the loop
//! rewrites run.json and emits `run:state` if anything changed. File I/O runs
//! on tokio's blocking pool ([`blocking`]) so it never stalls a runtime worker.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot, watch};

use crate::contract::{
    AppError, AppResult, MessageRole, NodeEvent, NodeEventKind, NodeRunState, NodeStatus,
    NodeSummary, RunMode, RunState, RunStatus,
};
use crate::run_store::{append_log, write_run_state};
use crate::runner::{run_turn, TurnOutcome, TurnSpec};
use crate::scheduler::{CancelEffect, Scheduler};
use crate::turns::{
    followup_status, followup_turn, initial_turn, merge_denials, record_followup, record_initial,
    save_message, RunCtx,
};

/// Attempts for the final run.json write.
const FINAL_WRITE_ATTEMPTS: u32 = 3;

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
    /// Status and reason of each node after its latest turn; the base a
    /// follow-up's outcome is combined with ([`followup_status`]).
    effective: HashMap<String, (NodeStatus, Option<String>)>,
    // Unbounded on purpose: senders are user actions (cancel, messages) and one
    // TurnDone per finished turn, and the loop drains continuously, so the
    // queue stays small; a bounded channel could block a turn's final report.
    tx: mpsc::UnboundedSender<LoopMsg>,
    rx: mpsc::UnboundedReceiver<LoopMsg>,
    persisted: Option<RunState>,
}

/// Run blocking file I/O on tokio's blocking pool.
pub async fn blocking<T, F>(f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| {
        Err(AppError::new(
            "internal",
            format!("blocking task failed: {e}"),
        ))
    })
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
        mode: RunMode::Headless,
        sessions: Vec::new(),
    }
}

impl RunLoop {
    pub fn new(ctx: Arc<RunCtx>, sched: Scheduler, state: RunState) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let initially_terminal = state
            .nodes
            .iter()
            .filter(|(_, s)| s.status.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();
        Self {
            ctx,
            sched,
            state,
            initially_terminal,
            kills: HashMap::new(),
            queues: HashMap::new(),
            effective: HashMap::new(),
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
            self.start_ready().await;
            self.sync_and_publish().await;
            if self.sched.is_done() {
                // Messages already in the channel (a follow-up request) still count.
                match self.rx.try_recv() {
                    Ok(msg) => {
                        self.handle(msg).await;
                        continue;
                    }
                    Err(_) => break,
                }
            }
            match self.rx.recv().await {
                Some(msg) => self.handle(msg).await,
                None => break,
            }
        }
        self.finish().await
    }

    async fn handle(&mut self, msg: LoopMsg) {
        match msg {
            LoopMsg::TurnDone {
                node_id,
                followup,
                outcome,
            } => self.on_turn_done(&node_id, followup, outcome).await,
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
                let result = self.on_message(&node_id, text).await;
                if reply.send(result).is_err() {
                    log::warn!("message to node {node_id}: the caller went away before the reply");
                }
            }
        }
    }

    fn kill(&self, id: &str) {
        if let Some(k) = self.kills.get(id) {
            if k.send(true).is_err() {
                log::warn!("kill node {id}: its turn already ended");
            }
        }
    }

    async fn start_ready(&mut self) {
        for id in self.sched.ready() {
            let now = Utc::now();
            if let Some(node) = self.state.nodes.get_mut(&id) {
                node.started_at = Some(now);
                node.status = NodeStatus::Running;
            }
            self.sync_and_publish().await;
            let (ctx, state, sched, node_id) = (
                self.ctx.clone(),
                self.state.clone(),
                self.sched.clone(),
                id.clone(),
            );
            let spec = blocking(move || initial_turn(&ctx, &state, &sched, &node_id, now)).await;
            match spec {
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
            let done = LoopMsg::TurnDone {
                node_id: node_id.clone(),
                followup,
                outcome,
            };
            if tx.send(done).is_err() {
                log::warn!("node {node_id}: the run loop ended before the turn reported");
            }
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

    /// Write result.md / summary.json / messages for a finished turn; returns
    /// the node's summary.json content (accumulated over follow-ups).
    async fn record(
        &self,
        id: &str,
        followup: bool,
        outcome: &TurnOutcome,
        status: NodeStatus,
        now: DateTime<Utc>,
    ) -> AppResult<NodeSummary> {
        let (ctx, id, outcome) = (self.ctx.clone(), id.to_string(), outcome.clone());
        blocking(move || {
            if followup {
                record_followup(&ctx, &id, &outcome, status, now)
            } else {
                record_initial(&ctx, &id, &outcome)
            }
        })
        .await
    }

    async fn on_turn_done(&mut self, id: &str, followup: bool, outcome: TurnOutcome) {
        self.kills.remove(id);
        let now = Utc::now();
        if followup {
            self.sched.release(id);
        }
        let (status, reason) = if followup {
            let base = self
                .effective
                .get(id)
                .cloned()
                .unwrap_or((outcome.status, outcome.reason.clone()));
            followup_status(base, &outcome)
        } else {
            (outcome.status, outcome.reason.clone())
        };
        self.effective
            .insert(id.to_string(), (status, reason.clone()));
        let recorded = self.record(id, followup, &outcome, status, now).await;
        if let Err(e) = &recorded {
            log::error!("recording node {id} of run {} failed: {e}", self.ctx.run_id);
        }
        if let Some(node) = self.state.nodes.get_mut(id) {
            node.session_id = outcome.session_id.clone().or(node.session_id.take());
            match recorded {
                Ok(summary) => {
                    node.cost_usd = summary.cost_usd;
                    node.num_turns = summary.num_turns;
                    node.permission_denials = summary.permission_denials;
                }
                Err(_) => {
                    node.permission_denials =
                        merge_denials(&node.permission_denials, &outcome.permission_denials);
                }
            }
            node.finished_at = Some(now);
        }
        let log_status = if followup {
            "followup".to_string()
        } else {
            status.as_str().to_string()
        };
        let (dir, run_id, node_id) = (
            self.ctx.graph_dir.clone(),
            self.ctx.run_id.clone(),
            id.to_string(),
        );
        let (cost, duration) = (outcome.cost_usd, outcome.duration_ms);
        let logged = blocking(move || {
            append_log(&dir, &run_id, &node_id, &log_status, cost, Some(duration))
        })
        .await;
        if let Err(e) = logged {
            log::error!("GRAPH_LOG append failed: {e}");
        }
        self.next_followup(id, &outcome);
        self.sched.on_finished(id, status, reason);
    }

    /// Start the next queued follow-up of `id`, or drop the queue if impossible.
    fn next_followup(&mut self, id: &str, outcome: &TurnOutcome) {
        let session = self.state.nodes.get(id).and_then(|n| n.session_id.clone());
        let started = self.state.nodes.get(id).and_then(|n| n.started_at);
        let Some(queue) = self.queues.get_mut(id) else {
            return;
        };
        let Some(text) = queue.pop_front() else {
            return;
        };
        let spec = match (outcome.status, session) {
            (NodeStatus::Cancelled, _) => None,
            (_, Some(sid)) => match followup_turn(&self.ctx, id, &sid, &text, started) {
                Ok(spec) => Some(spec),
                Err(e) => {
                    log::warn!("follow-up for node {id} not started: {e}");
                    None
                }
            },
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

    async fn save(&self, id: &str, text: &str) -> AppResult<()> {
        let (ctx, id, text) = (self.ctx.clone(), id.to_string(), text.to_string());
        blocking(move || save_message(&ctx, &id, MessageRole::User, &text)).await
    }

    async fn on_message(&mut self, id: &str, text: String) -> AppResult<()> {
        let status = self
            .sched
            .status(id)
            .ok_or_else(|| AppError::new("not_found", format!("node {id} is not in this run")))?;
        let session = self.state.nodes.get(id).and_then(|n| n.session_id.clone());
        let started = self.state.nodes.get(id).and_then(|n| n.started_at);
        match status {
            NodeStatus::Running | NodeStatus::Waiting => {
                // Persist first: a failed write leaves the queue untouched.
                self.save(id, &text).await?;
                self.sched.hold(id);
                self.queues
                    .entry(id.to_string())
                    .or_default()
                    .push_back(text.clone());
                self.emit_event(id, NodeEventKind::UserMessage, text);
                Ok(())
            }
            NodeStatus::Succeeded
            | NodeStatus::Failed
            | NodeStatus::Blocked
            | NodeStatus::Cancelled
                if session.is_some() =>
            {
                let sid = session.unwrap_or_default();
                let spec = followup_turn(&self.ctx, id, &sid, &text, started)?;
                self.save(id, &text).await?;
                let reason = self.sched.reason(id).map(str::to_string);
                self.effective.insert(id.to_string(), (status, reason));
                self.sched.reopen(id);
                if let Some(node) = self.state.nodes.get_mut(id) {
                    node.status = NodeStatus::Running;
                    node.reason = None;
                }
                self.emit_event(id, NodeEventKind::UserMessage, text);
                self.spawn_turn(spec, true);
                Ok(())
            }
            NodeStatus::Succeeded
            | NodeStatus::Failed
            | NodeStatus::Blocked
            | NodeStatus::Cancelled
            | NodeStatus::Pending
            | NodeStatus::Queued
            | NodeStatus::Skipped => Err(AppError::new("not_found", "node has not run yet")),
        }
    }

    fn sync_statuses(&mut self) {
        for (id, node) in self.state.nodes.iter_mut() {
            if let Some(status) = self.sched.status(id) {
                if status != node.status {
                    node.status = status;
                    node.reason = self.sched.reason(id).map(str::to_string);
                }
            }
        }
    }

    async fn write_state(&self) -> AppResult<()> {
        let (dir, state) = (self.ctx.graph_dir.clone(), self.state.clone());
        blocking(move || write_run_state(&dir, &state)).await
    }

    /// Copy scheduler statuses into the run state; persist and emit on change.
    /// `persisted` only advances when the write succeeded, so a failed write is
    /// retried on the next change.
    async fn sync_and_publish(&mut self) {
        self.sync_statuses();
        if self.persisted.as_ref() == Some(&self.state) {
            return;
        }
        match self.write_state().await {
            Ok(()) => self.persisted = Some(self.state.clone()),
            Err(e) => log::error!("writing run.json failed: {e}"),
        }
        self.ctx.emitter.run_state(&self.state);
    }

    async fn finish(mut self) -> RunState {
        self.sched.finalize();
        self.sync_statuses();
        let never_ran: Vec<(String, NodeStatus)> = self
            .state
            .nodes
            .iter()
            .filter(|(id, n)| {
                n.started_at.is_none()
                    && !self.initially_terminal.contains(*id)
                    && n.status.is_terminal()
            })
            .map(|(id, n)| (id.clone(), n.status))
            .collect();
        for (id, status) in never_ran {
            let (dir, run_id) = (self.ctx.graph_dir.clone(), self.ctx.run_id.clone());
            let text = status.as_str().to_string();
            let logged = blocking(move || append_log(&dir, &run_id, &id, &text, None, None)).await;
            if let Err(e) = logged {
                log::warn!("GRAPH_LOG append failed: {e}");
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
        for attempt in 1..=FINAL_WRITE_ATTEMPTS {
            match self.write_state().await {
                Ok(()) => {
                    self.persisted = Some(self.state.clone());
                    break;
                }
                Err(e) => {
                    log::error!(
                        "writing the final run.json (attempt {attempt}/{FINAL_WRITE_ATTEMPTS}) failed: {e}"
                    );
                    tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
                }
            }
        }
        self.ctx.emitter.run_state(&self.state);
        self.state
    }
}

//! One node of a session: prepare its message, deliver it with the verified
//! procedure ([`super::deliver`]), then wait for the reply's Stop hook.
//!
//! The node stays `queued` (event "delivering message…") until Claude Code
//! confirms the prompt with the `submit` hook; only then is it `running`. A
//! message that is not taken after the last cycle fails the node with
//! [`NOT_SUBMITTED`]; the session goes on unless `stop_on_failure`.

use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};

use crate::contract::{Model, Node, NodeEventKind, NodeStatus};
use crate::harness::adapter::StopSignal;
use crate::run_store::node_run_dir;
use crate::runner::model_flag;
use crate::terminal::deliver::{deliver, Delivery, Halt, Outcome, NOT_SUBMITTED};
use crate::terminal::hooks::{Hook, HookLine};
use crate::terminal::message::Typed;
use crate::terminal::node::{evaluate, prepare};
use crate::terminal::port::{PtyPort, Tick};
use crate::terminal::reply::write_reply;
use crate::terminal::session::{End, Session, CANCELLED, POLL};
use crate::terminal::{session_dir, TRANSCRIPT_FILE};

/// Node event while the message is on its way.
pub const DELIVERING: &str = "delivering message…";
/// After the Stop hook, wait until the output is quiet this long …
const SETTLE_QUIET: Duration = Duration::from_millis(250);
/// … but at most this long, so the reply's last redraw is in the transcript.
const SETTLE_MAX: Duration = Duration::from_millis(1500);
const SETTLE_STEP: Duration = Duration::from_millis(100);

type NodeEnd = (NodeStatus, Option<String>, Option<End>);

impl Session {
    pub(super) async fn run_node(&mut self, id: &str, started_at: DateTime<Utc>) -> NodeEnd {
        self.span = None;
        self.report(|i| i.current_node = Some(id.to_string()));
        self.event(NodeEventKind::Text, DELIVERING.to_string());
        let ctx = self.t.ctx.clone();
        let Ok(node) = ctx.node(id) else {
            return (NodeStatus::Failed, Some(format!("no node {id}")), None);
        };
        let typed = match prepare(&ctx, node, &self.done, &self.t.plan.node_ids) {
            Ok(t) => t,
            Err(e) => {
                let reason = format!("could not prepare the node: {}", e.message);
                return (NodeStatus::Failed, Some(reason), None);
            }
        };
        let (carried, from) = match self.deliver_node(id, node, &typed).await {
            Ok(Some(submitted)) => submitted,
            Ok(None) => return (NodeStatus::Failed, Some(NOT_SUBMITTED.to_string()), None),
            Err(halt) => return ended(halt_end(halt)),
        };
        self.node_msg(id, NodeStatus::Running, None, true);
        let title = &self.t.plan.title;
        self.event(
            NodeEventKind::Text,
            format!("sent node {id} to window {title}"),
        );
        let end = self
            .wait_stop(node, SystemTime::from(started_at), carried, typed.text())
            .await;
        if end.2.is_none() {
            self.settle().await;
        }
        self.span = self.save_reply(id, typed.text(), from).await;
        end
    }

    /// Wait (bounded) for the output to go quiet after the Stop hook.
    async fn settle(&mut self) {
        let t0 = Instant::now();
        while !self.quiet_for(SETTLE_QUIET) && t0.elapsed() < SETTLE_MAX {
            if !matches!(self.tick(SETTLE_STEP).await, Tick::Poll) {
                return;
            }
        }
    }

    /// reply.md / result.md of the turn from transcript offset `from` to now
    /// (written on a blocking thread: the transcript is replayed); returns
    /// the span (also when writing failed: it is logged).
    async fn save_reply(&self, id: &str, prompt: &str, from: u64) -> Option<(u64, u64)> {
        let pty = self.pty.as_ref()?;
        let to = pty.output.transcript_len();
        let sizes = pty.sizes();
        let ctx = &self.t.ctx;
        let transcript =
            session_dir(&ctx.graph_dir, &ctx.run_id, &self.t.plan.sid).join(TRANSCRIPT_FILE);
        let dir = node_run_dir(&ctx.graph_dir, &ctx.run_id, id);
        let prompt = prompt.to_string();
        let written = tokio::task::spawn_blocking(move || {
            write_reply(&dir, &transcript, (from, to), &sizes, &prompt)
        })
        .await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("writing the reply of node {id} failed: {e}"),
            Err(e) => log::warn!("writing the reply of node {id} failed: {e}"),
        }
        Some((from, to))
    }

    /// `/model` (if the node sets one) and the message. `Some((hooks seen
    /// right after the submit line, transcript length when the message's
    /// delivery began))` once submitted, `None` if never taken.
    async fn deliver_node(
        &mut self,
        id: &str,
        node: &Node,
        typed: &Typed,
    ) -> Result<Option<(Vec<Hook>, u64)>, Halt> {
        let shared = self.t.shared.clone();
        let _typing = shared.typing().await;
        self.poll_hooks(); // hooks of earlier, user-typed prompts do not count
        let Some(pty) = self.pty.clone() else {
            return Err(Halt::Exited(u32::MAX));
        };
        let title = self.t.plan.title.clone();
        let mut port = PtyPort {
            pty,
            hooks: &mut self.hooks,
            after_submit: Vec::new(),
            announced: &mut self.announced,
            kill: self.kill.clone(),
            emitter: self.t.ctx.emitter.clone(),
            run_id: self.t.ctx.run_id.clone(),
            node_id: id.to_string(),
        };
        let model = model_flag(node.model, Model::Inherit).filter(|_| self.behavior.types_model);
        if let Some(m) = model {
            let line = Typed::Line(format!("/model {m}"));
            if deliver(&mut port, &Delivery::model(&line, m, &title)).await?
                == Outcome::NotSubmitted
            {
                use crate::terminal::deliver::Port as _;
                port.event(
                    NodeEventKind::Stderr,
                    format!(
                        "warning: {} did not confirm /model {m}; continuing",
                        self.behavior.label
                    ),
                );
            }
        }
        let from = port.pty.output.transcript_len();
        let b = &self.behavior;
        let delivery = Delivery::prompt_with(typed, &title, b.expect.clone(), b.ready_quiet);
        match deliver(&mut port, &delivery).await? {
            Outcome::Submitted => Ok(Some((port.after_submit, from))),
            Outcome::NotSubmitted => Ok(None),
        }
    }

    /// Wait for the end of the reply (no timeout): the `stop` line of this
    /// turn (a Codex notify for a side turn does not count, see
    /// [`HookLine::stops_turn`]), or, for a quiet-mode harness, no output for
    /// its quiet time; `notify` lines become notification events.
    /// `carried`: hooks already read after `submit`.
    async fn wait_stop(
        &mut self,
        node: &Node,
        started: SystemTime,
        carried: Vec<Hook>,
        typed: &str,
    ) -> NodeEnd {
        let title = self.t.plan.title.clone();
        let label = self.behavior.label.clone();
        let submitted = Instant::now();
        let mut lines: Vec<HookLine> = carried
            .into_iter()
            .map(|hook| HookLine {
                hook,
                payload: None,
            })
            .collect();
        loop {
            if lines.iter().any(|l| l.hook == Hook::Notify) {
                self.event(
                    NodeEventKind::Notification,
                    format!("{label} is waiting for you in window {title}"),
                );
            }
            let stopped = match self.behavior.stop {
                StopSignal::Hook => lines.iter().any(|l| l.stops_turn(typed)),
                StopSignal::Quiet(d) => submitted.elapsed() >= d && self.quiet_for(d),
            };
            if stopped {
                let (status, reason) = evaluate(&self.t.ctx, node, started);
                return (status, reason, None);
            }
            match self.tick(POLL).await {
                Tick::Killed => return ended(End::Cancelled),
                Tick::Exited(code) => return ended(End::Exited(code)),
                Tick::Poll => {}
            }
            lines = self.hooks.poll_lines().unwrap_or_else(|e| {
                log::warn!("reading hooks.log failed: {e}");
                Vec::new()
            });
        }
    }
}

fn halt_end(halt: Halt) -> End {
    match halt {
        Halt::Killed => End::Cancelled,
        Halt::Exited(code) => End::Exited(code),
    }
}

/// The node's own status when the session ends while it runs.
fn ended(end: End) -> NodeEnd {
    let (status, reason) = match &end {
        End::Cancelled => (NodeStatus::Cancelled, CANCELLED.to_string()),
        End::Exited(code) => (
            NodeStatus::Failed,
            format!("the CLI exited (code {code}) before the node finished"),
        ),
        End::Failed(r) => (NodeStatus::Failed, r.clone()),
    };
    (status, Some(reason), Some(end))
}

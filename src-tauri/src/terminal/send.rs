//! One node of a session: prepare its message, deliver it with the verified
//! procedure ([`super::deliver`]), then follow its turn ([`super::turn`]:
//! done, a question, or a dialog on the way).
//!
//! The node stays `queued` (event "delivering message…") until the CLI
//! confirms the prompt; only then is it `running`. A message that is not
//! taken after the last cycle fails the node with [`NOT_SUBMITTED`]; the
//! session goes on unless `stop_on_failure`.

use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};

use crate::contract::{Model, Node, NodeEventKind, NodeStatus};
use crate::run_store::node_run_dir;
use crate::runner::model_flag;
use crate::terminal::deliver::{deliver, Delivery, Halt, Outcome, NOT_SUBMITTED};
use crate::terminal::hooks::HookLine;
use crate::terminal::message::Typed;
use crate::terminal::node::prepare;
use crate::terminal::port::{PtyPort, Tick};
use crate::terminal::reply::write_reply;
use crate::terminal::session::{End, NodeEnd, Session, CANCELLED};
use crate::terminal::turn::Turn;
use crate::terminal::{session_dir, TRANSCRIPT_FILE};

/// Node event while the message is on its way.
pub const DELIVERING: &str = "delivering message…";
/// After the Stop hook, wait until the output is quiet this long …
const SETTLE_QUIET: Duration = Duration::from_millis(250);
/// … but at most this long, so the reply's last redraw is in the transcript.
const SETTLE_MAX: Duration = Duration::from_millis(1500);
const SETTLE_STEP: Duration = Duration::from_millis(100);

impl Session {
    pub(super) async fn run_node(&mut self, id: &str, started_at: DateTime<Utc>) -> NodeEnd {
        self.span = None;
        if self.chain.is_some() {
            return self.run_chained_node(id, started_at).await;
        }
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
        let mark = self.pty.as_ref().map_or(0, |p| p.output.mark());
        let turn = Turn::new(
            id,
            typed.text(),
            from,
            mark,
            SystemTime::from(started_at),
            carried,
        );
        let end = self.turn(node, turn).await;
        if self.span.is_none() {
            // Cut short (kill, exit) before any reply was written.
            self.span = self.save_reply(id, typed.text(), from).await;
        }
        end
    }

    /// Wait (bounded) for the output to go quiet after the Stop hook.
    pub(super) async fn settle(&mut self) {
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
    pub(super) async fn save_reply(&self, id: &str, prompt: &str, from: u64) -> Option<(u64, u64)> {
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

    /// A delivery port on the session's pty for node `node_id`'s events.
    pub(super) fn port(&mut self, node_id: &str) -> Option<PtyPort<'_>> {
        let pty = self.pty.clone()?;
        Some(PtyPort {
            pty,
            hooks: &mut self.hooks,
            after_submit: Vec::new(),
            announced: &mut self.announced,
            kill: self.kill.clone(),
            emitter: self.t.ctx.emitter.clone(),
            run_id: self.t.ctx.run_id.clone(),
            node_id: node_id.to_string(),
        })
    }

    /// `/model` (if the node sets one) and the message. `Some((hook lines
    /// from the submit line on, transcript length when the message's delivery
    /// began))` once submitted, `None` if never taken.
    async fn deliver_node(
        &mut self,
        id: &str,
        node: &Node,
        typed: &Typed,
    ) -> Result<Option<(Vec<HookLine>, u64)>, Halt> {
        let shared = self.t.shared.clone();
        let _typing = shared.typing().await;
        self.poll_hooks(); // hooks of earlier, user-typed prompts do not count
        let title = self.t.plan.title.clone();
        let label = self.behavior.label.clone();
        let types_model = self.behavior.types_model;
        let (expect, ready) = (self.behavior.expect.clone(), self.behavior.ready_quiet);
        let Some(mut port) = self.port(id) else {
            return Err(Halt::Exited(u32::MAX));
        };
        let model = model_flag(node.model, Model::Inherit).filter(|_| types_model);
        if let Some(m) = model {
            let line = Typed::Line(format!("/model {m}"));
            if deliver(&mut port, &Delivery::model(&line, m, &title)).await?
                == Outcome::NotSubmitted
            {
                use crate::terminal::deliver::Port as _;
                port.event(
                    NodeEventKind::Stderr,
                    format!("warning: {label} did not confirm /model {m}; continuing"),
                );
            }
        }
        let from = port.pty.output.transcript_len();
        let delivery = Delivery::prompt_with(typed, &title, expect, ready);
        match deliver(&mut port, &delivery).await? {
            Outcome::Submitted => Ok(Some((port.after_submit, from))),
            Outcome::NotSubmitted => Ok(None),
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

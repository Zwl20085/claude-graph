//! One node of a session: prepare its message, deliver it with the verified
//! procedure ([`super::deliver`]), then wait for the reply's Stop hook.
//!
//! The node stays `queued` (event "delivering message…") until Claude Code
//! confirms the prompt with the `submit` hook; only then is it `running`. A
//! message that is not taken after the last cycle fails the node with
//! [`NOT_SUBMITTED`]; the session goes on unless `stop_on_failure`.

use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::contract::{Model, Node, NodeEventKind, NodeStatus};
use crate::runner::model_flag;
use crate::terminal::deliver::{deliver, Delivery, Halt, Outcome, NOT_SUBMITTED};
use crate::terminal::hooks::Hook;
use crate::terminal::message::Typed;
use crate::terminal::node::{evaluate, prepare};
use crate::terminal::port::{PtyPort, Tick};
use crate::terminal::session::{End, Session, CANCELLED, POLL};

/// Node event while the message is on its way.
pub const DELIVERING: &str = "delivering message…";

type NodeEnd = (NodeStatus, Option<String>, Option<End>);

impl Session {
    pub(super) async fn run_node(&mut self, id: &str, started_at: DateTime<Utc>) -> NodeEnd {
        self.report(|i| i.current_node = Some(id.to_string()));
        self.event(NodeEventKind::Text, DELIVERING.to_string());
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
        let carried = match self.deliver_node(id, node, &typed).await {
            Ok(Some(carried)) => carried,
            Ok(None) => return (NodeStatus::Failed, Some(NOT_SUBMITTED.to_string()), None),
            Err(halt) => return ended(halt_end(halt)),
        };
        self.node_msg(id, NodeStatus::Running, None, true);
        let title = &self.t.plan.title;
        self.event(
            NodeEventKind::Text,
            format!("sent node {id} to window {title}"),
        );
        self.wait_stop(node, SystemTime::from(started_at), carried)
            .await
    }

    /// `/model` (if the node sets one) and the message. `Some(hooks seen
    /// right after the submit line)` once submitted, `None` if never taken.
    async fn deliver_node(
        &mut self,
        id: &str,
        node: &Node,
        typed: &Typed,
    ) -> Result<Option<Vec<Hook>>, Halt> {
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
        if let Some(m) = model_flag(node.model, Model::Inherit) {
            let line = Typed::Line(format!("/model {m}"));
            if deliver(&mut port, &Delivery::model(&line, m, &title)).await?
                == Outcome::NotSubmitted
            {
                use crate::terminal::deliver::Port as _;
                port.event(
                    NodeEventKind::Stderr,
                    format!("warning: Claude Code did not confirm /model {m}; continuing"),
                );
            }
        }
        match deliver(&mut port, &Delivery::prompt(typed, &title)).await? {
            Outcome::Submitted => Ok(Some(port.after_submit)),
            Outcome::NotSubmitted => Ok(None),
        }
    }

    /// Wait for the reply's `stop` line (no timeout); `notify` lines become
    /// notification events. `carried`: hooks already read after `submit`.
    async fn wait_stop(&mut self, node: &Node, started: SystemTime, carried: Vec<Hook>) -> NodeEnd {
        let title = self.t.plan.title.clone();
        let mut hooks = carried;
        loop {
            if hooks.contains(&Hook::Notify) {
                self.event(
                    NodeEventKind::Notification,
                    format!("Claude Code is waiting for you in window {title}"),
                );
            }
            if hooks.contains(&Hook::Stop) {
                let (status, reason) = evaluate(&self.t.ctx, node, started);
                return (status, reason, None);
            }
            match self.tick(POLL).await {
                Tick::Killed => return ended(End::Cancelled),
                Tick::Exited(code) => return ended(End::Exited(code)),
                Tick::Poll => {}
            }
            hooks = self.poll_hooks();
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
            format!("Claude Code exited (code {code}) before the node finished"),
        ),
        End::Failed(r) => (NodeStatus::Failed, r.clone()),
    };
    (status, Some(reason), Some(end))
}

//! Preparing and recording node turns: the initial turn (brief from upstream
//! results) and follow-up turns (`--resume`, message on stdin), plus the files
//! each turn leaves in `runs/<run>/<node>/`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::claude_bin::{ClaudeCommand, ResolveEnv};
use crate::contract::{
    AppError, AppResult, Graph, HarnessProfile, MessageRole, Node, NodeMessage, NodeStatus,
    NodeSummary, RunState,
};
use crate::events::{dedup, Emitter};
use crate::graph_util::{is_valid_session_id, upstream_map};
use crate::harness::command::resolve_program;
use crate::harness::effective;
use crate::harness::env::EnvChanges;
use crate::harness::headless::with_profile_model;
use crate::prompt::{
    display_dir, render_brief, render_prompt, resolve_working_dir, upstream_input, UpstreamInput,
    INSTRUCTION,
};
use crate::run_store::{
    append_message, node_run_dir, read_result_md, read_summary, write_summary, PROMPT_FILE,
    RESULT_FILE,
};
use crate::runner::{build_args, final_status, Resume, TurnOutcome, TurnSpec};
use crate::scheduler::Scheduler;
use crate::store::{deliverables_dir, read_optional, write_atomic};

/// Positional instruction for a follow-up turn (message text goes on stdin).
pub const FOLLOWUP_INSTRUCTION: &str =
    "Continue the node task with the follow-up message on stdin. Keep the same finish protocol.";

/// Per-run constants shared by the loop and its node tasks.
pub struct RunCtx {
    pub graph_dir: PathBuf,
    pub graph: Graph,
    pub run_id: String,
    pub claude: ClaudeCommand,
    pub emitter: Arc<dyn Emitter>,
    /// Harness profiles of this machine at run start (built-ins merged with
    /// overrides, then customs).
    pub harnesses: Vec<HarnessProfile>,
    /// The app binary, Codex's notify program (`<exe> --notify <hooks.log>`).
    pub notify_exe: PathBuf,
}

impl RunCtx {
    pub fn node(&self, id: &str) -> AppResult<&Node> {
        self.graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or_else(|| AppError::new("not_found", format!("no node {id:?} in the graph")))
    }

    /// The profile `id` (validated at run start).
    pub fn harness(&self, id: &str) -> AppResult<&HarnessProfile> {
        crate::harness::find(&self.harnesses, id).ok_or_else(|| {
            AppError::new(
                "invalid_input",
                format!("harness {id} is not defined on this machine"),
            )
        })
    }

    /// The node's claude-kind profile env (expanded) and model, for the
    /// headless engine.
    fn headless_harness(&self, node: &Node) -> AppResult<(EnvChanges, Option<String>)> {
        let profile = self.harness(effective(node, &self.graph.settings))?;
        let env = crate::harness::env::process_env(profile)?;
        Ok((env, profile.model.clone()))
    }

    /// The claude command of the node's profile (its own command, if set).
    fn headless_claude(&self, node: &Node) -> AppResult<ClaudeCommand> {
        let profile = self.harness(effective(node, &self.graph.settings))?;
        match &profile.command {
            Some(c) => resolve_program(c, &ResolveEnv::from_process(None)),
            None => Ok(self.claude.clone()),
        }
    }

    fn out_dir(&self, node_id: &str) -> PathBuf {
        node_run_dir(&self.graph_dir, &self.run_id, node_id)
    }

    fn required(&self, node: &Node) -> Vec<PathBuf> {
        let base = deliverables_dir(&self.graph_dir, &node.id);
        node.deliverables
            .iter()
            .map(|d| d.path.trim())
            .filter(|p| !p.is_empty())
            .map(|p| base.join(p))
            .collect()
    }
}

fn status_word(status: NodeStatus) -> String {
    status.as_str().to_string()
}

/// `before` followed by the tools of `new` not already in it.
pub fn merge_denials(before: &[String], new: &[String]) -> Vec<String> {
    dedup(before.iter().chain(new).cloned())
}

/// Inputs from every direct upstream node: this run's result if it ran here,
/// else the latest successful earlier run the scheduler found.
fn upstream_inputs(
    ctx: &RunCtx,
    state: &RunState,
    sched: &Scheduler,
    node: &Node,
) -> AppResult<Vec<UpstreamInput>> {
    let shown = display_dir(&ctx.graph_dir);
    let ups = upstream_map(&ctx.graph)
        .remove(&node.id)
        .unwrap_or_default();
    ctx.graph
        .nodes
        .iter()
        .filter(|n| ups.contains(&n.id))
        .map(|up| {
            let (run_id, status) = match state.nodes.get(&up.id) {
                Some(s) => (Some(ctx.run_id.as_str()), status_word(s.status)),
                None => (
                    sched.prior_run_of(&up.id),
                    status_word(NodeStatus::Succeeded),
                ),
            };
            let result = match run_id {
                Some(r) => read_result_md(&ctx.graph_dir, r, &up.id)?,
                None => None,
            };
            Ok(upstream_input(&shown, up, &status, result.as_deref()))
        })
        .collect()
}

/// Session to resume for `continue_session_of`: this run's, else the earlier run's.
fn resume_session(
    ctx: &RunCtx,
    state: &RunState,
    sched: &Scheduler,
    node: &Node,
) -> Option<String> {
    let up = node.continue_session_of.as_deref()?.trim();
    if up.is_empty() {
        return None;
    }
    if let Some(s) = state.nodes.get(up) {
        return s.session_id.clone();
    }
    let prior = sched.prior_run_of(up)?;
    read_summary(&ctx.graph_dir, prior, up)
        .ok()
        .flatten()
        .and_then(|s| s.session_id)
}

/// A session id from run.json or summary.json, checked before it reaches `--resume`.
pub fn checked_session(id: &str) -> AppResult<&str> {
    if is_valid_session_id(id) {
        Ok(id)
    } else {
        Err(AppError::new(
            "invalid_input",
            format!("refusing to resume session {id:?}: not a CLI session id"),
        ))
    }
}

/// Status of a node after a follow-up turn, given its status before it
/// (`base`). Follow-ups never downgrade a succeeded node unless the follow-up
/// itself reports `## Status: BLOCKED`; a cancelled follow-up cancels the node.
/// A blocked node stays blocked (with its reason) when the follow-up reply has
/// no `## Status:` line at all. Otherwise a node that had not succeeded takes
/// the follow-up's own evaluation.
pub fn followup_status(
    base: (NodeStatus, Option<String>),
    outcome: &TurnOutcome,
) -> (NodeStatus, Option<String>) {
    let status_line = outcome.final_text.as_deref().and_then(final_status);
    let blocked = status_line.as_ref().is_some_and(|(done, _)| !done);
    match (outcome.status, base.0) {
        (NodeStatus::Cancelled, _) => (NodeStatus::Cancelled, outcome.reason.clone()),
        (_, NodeStatus::Succeeded) if !blocked => (NodeStatus::Succeeded, None),
        (_, NodeStatus::Blocked) if status_line.is_none() => base,
        _ => (outcome.status, outcome.reason.clone()),
    }
}

/// Write prompt.md and build the first turn of `node_id`.
pub fn initial_turn(
    ctx: &RunCtx,
    state: &RunState,
    sched: &Scheduler,
    node_id: &str,
    now: DateTime<Utc>,
) -> AppResult<TurnSpec> {
    let node = ctx.node(node_id)?;
    let shown = display_dir(&ctx.graph_dir);
    let brief = render_brief(
        &shown,
        &ctx.graph,
        node,
        &upstream_inputs(ctx, state, sched, node)?,
    );
    let prompt = render_prompt(&ctx.run_id, &now.to_rfc3339(), &brief);
    write_atomic(&ctx.out_dir(node_id).join(PROMPT_FILE), prompt.as_bytes())?;
    std::fs::create_dir_all(deliverables_dir(&ctx.graph_dir, node_id))?;
    let resume = resume_session(ctx, state, sched, node);
    // continue_session_of forks the upstream's session, leaving it untouched.
    let resume = match resume.as_deref() {
        Some(id) => Some(Resume {
            session_id: checked_session(id)?,
            fork: true,
        }),
        None => None,
    };
    let (env, model) = ctx.headless_harness(node)?;
    Ok(TurnSpec {
        run_id: ctx.run_id.clone(),
        node_id: node_id.to_string(),
        claude: ctx.headless_claude(node)?,
        args: with_profile_model(
            build_args(
                node,
                &ctx.graph.settings,
                &ctx.graph_dir,
                resume,
                INSTRUCTION,
            ),
            node,
            model,
        ),
        env,
        stdin_text: prompt,
        cwd: resolve_working_dir(&ctx.graph_dir, node),
        out_dir: ctx.out_dir(node_id),
        required: ctx.required(node),
        fresh_since: None,
    })
}

/// A follow-up turn resuming `session_id` with `text` on stdin. Deliverables
/// written since `node_started` (the node's first start in this run) count.
pub fn followup_turn(
    ctx: &RunCtx,
    node_id: &str,
    session_id: &str,
    text: &str,
    node_started: Option<DateTime<Utc>>,
) -> AppResult<TurnSpec> {
    let node = ctx.node(node_id)?;
    let resume = Resume {
        session_id: checked_session(session_id)?,
        fork: false,
    };
    let (env, model) = ctx.headless_harness(node)?;
    Ok(TurnSpec {
        run_id: ctx.run_id.clone(),
        node_id: node_id.to_string(),
        claude: ctx.headless_claude(node)?,
        args: with_profile_model(
            build_args(
                node,
                &ctx.graph.settings,
                &ctx.graph_dir,
                Some(resume),
                FOLLOWUP_INSTRUCTION,
            ),
            node,
            model,
        ),
        env,
        stdin_text: text.to_string(),
        cwd: resolve_working_dir(&ctx.graph_dir, node),
        out_dir: ctx.out_dir(node_id),
        required: ctx.required(node),
        fresh_since: node_started.map(SystemTime::from),
    })
}

pub fn save_message(ctx: &RunCtx, node_id: &str, role: MessageRole, text: &str) -> AppResult<()> {
    let message = NodeMessage {
        role,
        text: text.to_string(),
        ts: Utc::now(),
    };
    append_message(&ctx.graph_dir, &ctx.run_id, node_id, &message)
}

fn summary_of(outcome: &TurnOutcome) -> NodeSummary {
    NodeSummary {
        session_id: outcome.session_id.clone(),
        cost_usd: outcome.cost_usd,
        duration_ms: Some(outcome.duration_ms),
        num_turns: outcome.num_turns,
        is_error: outcome.is_error,
        status: outcome.status,
        permission_denials: outcome.permission_denials.clone(),
    }
}

/// result.md, summary.json and the assistant message after the first turn.
/// Returns the summary written.
pub fn record_initial(
    ctx: &RunCtx,
    node_id: &str,
    outcome: &TurnOutcome,
) -> AppResult<NodeSummary> {
    let dir = ctx.out_dir(node_id);
    if let Some(text) = &outcome.final_text {
        write_atomic(&dir.join(RESULT_FILE), text.as_bytes())?;
        save_message(ctx, node_id, MessageRole::Assistant, text)?;
    }
    let summary = summary_of(outcome);
    write_summary(&ctx.graph_dir, &ctx.run_id, node_id, &summary)?;
    Ok(summary)
}

fn add<T: std::ops::Add<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    }
}

fn count_followups(result_md: &str) -> usize {
    result_md
        .lines()
        .filter(|l| l.starts_with("## Follow-up "))
        .count()
}

/// Append the follow-up's final text to result.md and accumulate summary.json
/// (with `status`, the node's status after the follow-up, see [`followup_status`]).
/// Returns the accumulated summary.
pub fn record_followup(
    ctx: &RunCtx,
    node_id: &str,
    outcome: &TurnOutcome,
    status: NodeStatus,
    now: DateTime<Utc>,
) -> AppResult<NodeSummary> {
    let dir = ctx.out_dir(node_id);
    let previous = read_optional(&dir.join(RESULT_FILE))?.unwrap_or_default();
    let text = outcome.final_text.clone().unwrap_or_default();
    let n = count_followups(&previous) + 1;
    let section = format!("\n\n---\n## Follow-up {n} ({})\n\n{text}", now.to_rfc3339());
    write_atomic(
        &dir.join(RESULT_FILE),
        format!("{previous}{section}").as_bytes(),
    )?;
    if !text.is_empty() {
        save_message(ctx, node_id, MessageRole::Assistant, &text)?;
    }
    let before = read_summary(&ctx.graph_dir, &ctx.run_id, node_id)?;
    let turn = NodeSummary {
        status,
        ..summary_of(outcome)
    };
    let merged = match before {
        Some(b) => NodeSummary {
            session_id: turn.session_id.or(b.session_id),
            cost_usd: add(b.cost_usd, turn.cost_usd),
            duration_ms: add(b.duration_ms, turn.duration_ms),
            num_turns: add(b.num_turns, turn.num_turns),
            is_error: turn.is_error,
            status: turn.status,
            permission_denials: merge_denials(&b.permission_denials, &turn.permission_denials),
        },
        None => turn,
    };
    write_summary(&ctx.graph_dir, &ctx.run_id, node_id, &merged)?;
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_up_counting_and_sums() {
        assert_eq!(
            count_followups("a\n## Follow-up 1 (t)\nb\n## Follow-up 2 (t)"),
            2
        );
        assert_eq!(count_followups("plain"), 0);
        assert_eq!(add(Some(1.5), Some(2.0)), Some(3.5));
        assert_eq!(add(None, Some(2u32)), Some(2));
        assert_eq!(add::<u32>(None, None), None);
        assert_eq!(status_word(NodeStatus::Succeeded), "succeeded");
    }

    fn outcome(status: NodeStatus, text: &str) -> TurnOutcome {
        TurnOutcome {
            status,
            reason: (status != NodeStatus::Succeeded).then(|| "turn reason".to_string()),
            session_id: None,
            cost_usd: None,
            num_turns: None,
            duration_ms: 1,
            is_error: false,
            final_text: Some(text.to_string()),
            model: None,
            permission_denials: Vec::new(),
        }
    }

    #[test]
    fn follow_ups_never_downgrade_a_succeeded_node_unless_blocked() {
        let ok = (NodeStatus::Succeeded, None);
        let no_status = outcome(NodeStatus::Failed, "just an answer");
        assert_eq!(followup_status(ok.clone(), &no_status), ok);
        let exit_error = outcome(NodeStatus::Failed, "## Status: DONE");
        assert_eq!(followup_status(ok.clone(), &exit_error), ok);
        let blocked = outcome(NodeStatus::Blocked, "x\n## Status: BLOCKED\nno data");
        assert_eq!(
            followup_status(ok.clone(), &blocked),
            (NodeStatus::Blocked, Some("turn reason".into()))
        );
        let cancelled = outcome(NodeStatus::Cancelled, "");
        assert_eq!(followup_status(ok, &cancelled).0, NodeStatus::Cancelled);
        // A failed node is fixed by a successful follow-up, stays failed otherwise.
        let failed = (NodeStatus::Failed, Some("old".into()));
        let done = outcome(NodeStatus::Succeeded, "## Status: DONE");
        assert_eq!(
            followup_status(failed.clone(), &done),
            (NodeStatus::Succeeded, None)
        );
        assert_eq!(followup_status(failed, &no_status).0, NodeStatus::Failed);
        // A blocked node: DONE promotes it, another BLOCKED keeps it blocked.
        let was_blocked = (NodeStatus::Blocked, Some("May I?".into()));
        assert_eq!(
            followup_status(was_blocked.clone(), &done),
            (NodeStatus::Succeeded, None)
        );
        assert_eq!(
            followup_status(was_blocked.clone(), &blocked).0,
            NodeStatus::Blocked
        );
        // A reply without any status line never downgrades a blocked node.
        assert_eq!(
            followup_status(was_blocked.clone(), &no_status),
            was_blocked
        );
        let no_text = TurnOutcome {
            final_text: None,
            ..no_status.clone()
        };
        assert_eq!(followup_status(was_blocked.clone(), &no_text), was_blocked);
    }

    #[test]
    fn denials_merge_without_repeats_in_first_seen_order() {
        let before = vec!["WebSearch".to_string(), "Bash".to_string()];
        let new = vec!["Bash".to_string(), "Write".to_string()];
        assert_eq!(
            merge_denials(&before, &new),
            vec!["WebSearch", "Bash", "Write"]
        );
        assert_eq!(merge_denials(&[], &new), new);
        assert_eq!(status_word(NodeStatus::Blocked), "blocked");
    }

    #[test]
    fn session_ids_are_checked_before_resume() {
        assert!(checked_session("0f8fad5b-d9cb-469f-a165-70867728950e").is_ok());
        assert_eq!(
            checked_session("--dangerously-skip-permissions")
                .unwrap_err()
                .code,
            "invalid_input"
        );
    }
}

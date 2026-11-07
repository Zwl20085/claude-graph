//! One node inside a terminal session: its brief, its message, the
//! deliverables check after the Stop hook, and the files it leaves
//! (`nodes/<id>/brief.md`, `runs/<run>/<id>/prompt.md`, `summary.json`, the
//! GRAPH_LOG line).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::contract::{AppResult, Node, NodeStatus};
use crate::graph_util::upstream_map;
use crate::prompt::{deliverables_display, display_dir, upstream_input};
use crate::run_store::{append_log, node_run_dir, PROMPT_FILE, SUMMARY_FILE};
use crate::runner::check_deliverables;
use crate::store::{deliverables_dir, node_dir, write_atomic, write_json};
use crate::terminal::message::{message_for, terminal_brief, Typed};
use crate::terminal::TerminalNodeSummary;
use crate::turns::RunCtx;

/// Status word for an upstream node that did not run in this run.
const NOT_RUN_HERE: &str = "not run in this run";

/// Write the node's terminal brief (`nodes/<id>/brief.md`, copied to the run's
/// `prompt.md`), create its deliverables folder, and return the message.
/// `done`: statuses of the nodes this session already handled in this run.
pub fn prepare(ctx: &RunCtx, node: &Node, done: &BTreeMap<String, NodeStatus>) -> AppResult<Typed> {
    let shown = display_dir(&ctx.graph_dir);
    let ups = upstream_map(&ctx.graph)
        .remove(&node.id)
        .unwrap_or_default();
    let upstream: Vec<_> = ctx
        .graph
        .nodes
        .iter()
        .filter(|n| ups.contains(&n.id))
        .map(|up| {
            let status = done.get(&up.id).map_or(NOT_RUN_HERE, |s| s.as_str());
            upstream_input(&shown, up, status, None)
        })
        .collect();
    let brief = terminal_brief(&shown, &ctx.graph, node, &upstream);
    let brief_path = node_dir(&ctx.graph_dir, &node.id).join("brief.md");
    write_atomic(&brief_path, brief.as_bytes())?;
    let run_dir = node_run_dir(&ctx.graph_dir, &ctx.run_id, &node.id);
    write_atomic(&run_dir.join(PROMPT_FILE), brief.as_bytes())?;
    std::fs::create_dir_all(deliverables_dir(&ctx.graph_dir, &node.id))?;
    let brief_shown = format!("{shown}/nodes/{}/brief.md", node.id);
    Ok(message_for(
        node,
        &brief_shown,
        &deliverables_display(&shown, &node.id),
    ))
}

/// Required deliverables present and written since `started` (2 s slack):
/// succeeded; else failed with `deliverables missing: a, b`.
pub fn evaluate(ctx: &RunCtx, node: &Node, started: SystemTime) -> (NodeStatus, Option<String>) {
    let base = deliverables_dir(&ctx.graph_dir, &node.id);
    let missing: Vec<&str> = node
        .deliverables
        .iter()
        .map(|d| d.path.trim())
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let (absent, stale) = check_deliverables(&[base.join(p)], started);
            !absent.is_empty() || !stale.is_empty()
        })
        .collect();
    if missing.is_empty() {
        (NodeStatus::Succeeded, None)
    } else {
        (
            NodeStatus::Failed,
            Some(format!("deliverables missing: {}", missing.join(", "))),
        )
    }
}

/// One finished node of a session.
pub struct Finished<'a> {
    pub sid: &'a str,
    pub node_id: &'a str,
    pub status: NodeStatus,
    pub reason: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

/// summary.json and the GRAPH_LOG line (cost `-`).
pub fn record(ctx: &RunCtx, f: &Finished<'_>) -> AppResult<()> {
    let duration = (f.finished_at - f.started_at)
        .to_std()
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let summary = TerminalNodeSummary {
        session: f.sid.to_string(),
        started_at: f.started_at,
        finished_at: f.finished_at,
        status: f.status,
        reason: f.reason.clone(),
        session_id: None,
        cost_usd: None,
        duration_ms: Some(duration),
        num_turns: None,
        is_error: f.status != NodeStatus::Succeeded,
        permission_denials: Vec::new(),
    };
    let path: PathBuf = node_run_dir(&ctx.graph_dir, &ctx.run_id, f.node_id).join(SUMMARY_FILE);
    write_json(&path, &summary)?;
    append_log(
        &ctx.graph_dir,
        &ctx.run_id,
        f.node_id,
        f.status.as_str(),
        None,
        Some(duration),
    )
}

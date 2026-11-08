//! `graph_report`: the agent says how a handed-out node ended. The app checks
//! the required deliverables itself and writes the node's record exactly as a
//! terminal-mode run does: reply.md / result.md, summary.json, the GRAPH_LOG
//! line and, for a blocked node with a question, questions.jsonl.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::contract::{
    AppError, AppResult, Graph, Node, NodeRunState, NodeStatus, PromptSource, QuestionKind,
    QuestionRecord, RunState,
};
use crate::contract_agent::{AgentMeta, AgentNodeSummary, AgentReport};
use crate::run_store::{append_log, node_run_dir, RESULT_FILE, SUMMARY_FILE};
use crate::runner::check_deliverables;
use crate::store::{deliverables_dir, read_graph, write_atomic, write_json};
use crate::terminal::questions;
use crate::terminal::reply::REPLY_FILE;

use super::next::require_running;
use super::open::{finish_if_settled, statuses};
use super::pick;
use super::run::{self, AgentRun};
use super::{cut_bytes, first_line, run_word, Caller, NOTE_MAX, SUMMARY_MAX};

/// What the agent reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Failed,
    Blocked,
}

impl Outcome {
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "done" | "succeeded" | "success" => Some(Outcome::Done),
            "failed" | "fail" => Some(Outcome::Failed),
            "blocked" | "waiting" => Some(Outcome::Blocked),
            _ => None,
        }
    }
}

/// A `graph_report` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportInput {
    pub node_id: String,
    pub outcome: Outcome,
    pub summary: String,
    pub question: Option<String>,
    pub deliverables_written: Vec<String>,
}

/// Reason of skipped dependents after `stop_on_failure`.
const STOPPED_AFTER: &str = "run stopped after a failure (stop_on_failure)";

/// Required deliverables missing or not written since `started` (2 s slack).
pub fn missing_deliverables(dir: &Path, node: &Node, started: SystemTime) -> Vec<String> {
    let base = deliverables_dir(dir, &node.id);
    node.deliverables
        .iter()
        .map(|d| d.path.trim())
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let (absent, stale) = check_deliverables(&[base.join(p)], started);
            !absent.is_empty() || !stale.is_empty()
        })
        .map(str::to_string)
        .collect()
}

/// Status and reason for the outcome.
fn judge(
    dir: &Path,
    node: &Node,
    input: &ReportInput,
    started: SystemTime,
) -> (NodeStatus, Option<String>) {
    match input.outcome {
        Outcome::Done => {
            let missing = missing_deliverables(dir, node, started);
            if missing.is_empty() {
                (NodeStatus::Succeeded, None)
            } else {
                let reason = format!("deliverables missing: {}", missing.join(", "));
                (NodeStatus::Failed, Some(reason))
            }
        }
        Outcome::Failed => {
            let reason = first_line(&input.summary, 300);
            let reason = if reason.is_empty() {
                "reported failed".to_string()
            } else {
                reason
            };
            (NodeStatus::Failed, Some(reason))
        }
        Outcome::Blocked => {
            let text = input
                .question
                .as_deref()
                .filter(|q| !q.trim().is_empty())
                .unwrap_or(&input.summary);
            let reason = first_line(text, 300);
            let reason = if reason.is_empty() {
                "blocked".to_string()
            } else {
                reason
            };
            (NodeStatus::Waiting, Some(reason))
        }
    }
}

fn reply_text(input: &ReportInput) -> String {
    let summary = cut_bytes(input.summary.trim(), SUMMARY_MAX);
    let mut text = if summary.is_empty() {
        "(no summary)".to_string()
    } else {
        summary.to_string()
    };
    if let Some(q) = input.question.as_deref().filter(|q| !q.trim().is_empty()) {
        text.push_str("\n\nQuestion: ");
        text.push_str(q.trim());
    }
    text.push('\n');
    text
}

/// Paths the agent says it wrote, relative ones against the deliverables folder.
fn written(dir: &Path, node_id: &str, list: &[String]) -> Vec<String> {
    let base = deliverables_dir(dir, node_id);
    list.iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .take(200)
        .map(|p| {
            let path = PathBuf::from(p);
            let full = if path.is_absolute() {
                path
            } else {
                base.join(path)
            };
            let mark = if full.is_file() { "" } else { " (not found)" };
            format!("{}{mark}", crate::prompt::display_dir(&full))
        })
        .collect()
}

struct Record<'a> {
    dir: &'a Path,
    run_id: &'a str,
    node: &'a Node,
    status: NodeStatus,
    reason: Option<String>,
    started: DateTime<Utc>,
    finished: DateTime<Utc>,
    client: &'a str,
}

fn write_record(r: &Record<'_>, input: &ReportInput) -> AppResult<usize> {
    let folder = node_run_dir(r.dir, r.run_id, &r.node.id);
    let reply = reply_text(input);
    write_atomic(&folder.join(REPLY_FILE), reply.as_bytes())?;
    write_atomic(&folder.join(RESULT_FILE), reply.as_bytes())?;
    if let Some(q) = input.question.as_deref().filter(|q| !q.trim().is_empty()) {
        let n = questions::read(&folder).len() + 1;
        let record = QuestionRecord {
            id: format!("{}-q{n}", r.node.id),
            ts: r.finished,
            kind: QuestionKind::Question,
            source: PromptSource::Reply,
            question: q.trim().chars().take(NOTE_MAX).collect(),
            header: None,
            options: Vec::new(),
            selected: None,
            offer: false,
            answer: None,
        };
        questions::add(&folder, record)?;
    }
    let asked = questions::read(&folder).len();
    let duration = (r.finished - r.started)
        .to_std()
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let summary = AgentNodeSummary {
        session: "agent".into(),
        started_at: r.started,
        finished_at: r.finished,
        status: r.status,
        reason: r.reason.clone(),
        session_id: None,
        cost_usd: None,
        duration_ms: Some(duration),
        num_turns: None,
        is_error: r.status != NodeStatus::Succeeded,
        permission_denials: Vec::new(),
        questions: asked,
        waited_for_user: r.status == NodeStatus::Waiting || asked > 0,
        deliverables_written: written(r.dir, &r.node.id, &input.deliverables_written),
        client: r.client.to_string(),
    };
    write_json(&folder.join(SUMMARY_FILE), &summary)?;
    if r.status != NodeStatus::Waiting {
        append_log(
            r.dir,
            r.run_id,
            &r.node.id,
            r.status.as_str(),
            None,
            Some(duration),
        )?;
    }
    Ok(asked)
}

fn set_node(state: RunState, id: &str, n: NodeRunState) -> RunState {
    let mut nodes = state.nodes;
    nodes.insert(id.to_string(), n);
    RunState { nodes, ..state }
}

fn skip(state: RunState, ids: &[String], reason: &str) -> RunState {
    ids.iter()
        .fold(state, |s, id| match s.nodes.get(id).cloned() {
            Some(n) => set_node(
                s,
                id,
                NodeRunState {
                    status: NodeStatus::Skipped,
                    reason: Some(reason.to_string()),
                    ..n
                },
            ),
            None => s,
        })
}

/// Dependents of a failed node are skipped; `stop_on_failure` skips every
/// pending node.
fn propagate(graph: &Graph, state: RunState, id: &str) -> RunState {
    let deps = pick::open_dependents(graph, &statuses(&state), id);
    let state = skip(state, &deps, &format!("upstream {id} failed"));
    if !graph.settings.stop_on_failure {
        return state;
    }
    let pending: Vec<String> = state
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.status, NodeStatus::Pending | NodeStatus::Queued))
        .map(|(k, _)| k.clone())
        .collect();
    skip(state, &pending, STOPPED_AFTER)
}

/// `graph_report`.
pub fn report(
    dir: &Path,
    run_id: &str,
    caller: &Caller,
    input: &ReportInput,
) -> AppResult<AgentReport> {
    let graph = read_graph(dir)?;
    let _lock = run::lock_run(dir, run_id)?;
    let run = run::read_agent(dir, run_id)?;
    require_running(&run)?;
    let id = input.node_id.trim();
    let current =
        run.state.nodes.get(id).cloned().ok_or_else(|| {
            AppError::new("not_found", format!("node {id:?} is not in run {run_id}"))
        })?;
    if !matches!(current.status, NodeStatus::Running | NodeStatus::Waiting) {
        return Err(AppError::new(
            "invalid_input",
            format!(
                "node {id} is {}; only a node handed out by graph_next can be reported",
                current.status.as_str()
            ),
        ));
    }
    let node = graph.nodes.iter().find(|n| n.id == id).ok_or_else(|| {
        AppError::new("not_found", format!("node {id} is no longer in the graph"))
    })?;
    let started = current.started_at.unwrap_or_else(Utc::now);
    let finished = Utc::now();
    let (status, reason) = judge(dir, node, input, SystemTime::from(started));
    write_record(
        &Record {
            dir,
            run_id,
            node,
            status,
            reason: reason.clone(),
            started,
            finished,
            client: &caller.client,
        },
        input,
    )?;
    let updated = NodeRunState {
        status,
        reason: reason.clone(),
        finished_at: (status != NodeStatus::Waiting).then_some(finished),
        ..current
    };
    let state = set_node(run.state, id, updated);
    let state = if status == NodeStatus::Failed {
        propagate(&graph, state, id)
    } else {
        state
    };
    let state = finish_if_settled(state);
    let line = match &reason {
        Some(r) => format!("reported {}: {r}", status.as_str()),
        None => format!(
            "reported {}: {}",
            status.as_str(),
            first_line(&input.summary, 200)
        ),
    };
    run::log(dir, run_id, Some(id), "report", &line);
    let run = AgentRun {
        state,
        meta: AgentMeta {
            last_seen: finished,
            ..run.meta
        },
    };
    run::write(dir, &run)?;
    let done = run.state.status != crate::contract::RunStatus::Running;
    if done {
        let text = format!("finished: {}", run_word(run.state.status));
        run::log(dir, run_id, None, "run", &text);
    }
    Ok(AgentReport {
        run_id: run_id.to_string(),
        node_id: id.to_string(),
        status,
        reason,
        ready: if done {
            Vec::new()
        } else {
            pick::ready(&graph, &statuses(&run.state))
        },
        done,
    })
}

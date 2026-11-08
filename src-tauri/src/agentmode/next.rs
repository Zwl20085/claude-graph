//! `graph_next`: hand out the ready node(s) of an agent run.

use std::path::Path;

use chrono::Utc;

use crate::contract::{AppError, AppResult, Graph, NodeRunState, NodeStatus, RunState, RunStatus};
use crate::contract_agent::{AgentBlocked, AgentBrief, AgentNext};
use crate::run_store::node_run_dir;
use crate::store::read_graph;
use crate::terminal::questions;

use super::brief::{paths_of, write_brief};
use super::open::{finish_if_settled, status_of, statuses};
use super::pick;
use super::run::{self, AgentRun};
use super::{Caller, NEXT_MAX};

/// Reason of nodes skipped because they can never become ready.
const STRANDED: &str = "never became ready";
/// Reason of run nodes no longer in graph.json.
const REMOVED: &str = "removed from the graph";

fn set(state: RunState, id: &str, f: impl FnOnce(NodeRunState) -> NodeRunState) -> RunState {
    let mut nodes = state.nodes;
    if let Some(n) = nodes.remove(id) {
        nodes.insert(id.to_string(), f(n));
    }
    RunState { nodes, ..state }
}

fn reset(n: NodeRunState) -> NodeRunState {
    NodeRunState {
        status: NodeStatus::Pending,
        reason: None,
        started_at: None,
        finished_at: None,
        ..n
    }
}

/// Retry `id` (failed or blocked): it and its skipped dependents are pending again.
fn retry(graph: &Graph, state: RunState, id: &str) -> AppResult<RunState> {
    let current = state
        .nodes
        .get(id)
        .ok_or_else(|| AppError::new("not_found", format!("node {id:?} is not in this run")))?
        .status;
    if !matches!(
        current,
        NodeStatus::Failed | NodeStatus::Waiting | NodeStatus::Blocked
    ) {
        return Err(AppError::new(
            "invalid_input",
            format!(
                "node {id} is {}; only a node reported failed or blocked can be retried",
                current.as_str()
            ),
        ));
    }
    let reopen = pick::skipped_dependents(graph, &statuses(&state), id);
    let state = set(state, id, reset);
    Ok(reopen.iter().fold(state, |s, dep| set(s, dep, reset)))
}

/// Nodes gone from graph.json and nodes that can never start are settled.
fn tidy(graph: &Graph, state: RunState) -> RunState {
    let gone: Vec<String> = state
        .nodes
        .iter()
        .filter(|(id, n)| {
            !graph.nodes.iter().any(|g| &g.id == *id)
                && matches!(n.status, NodeStatus::Pending | NodeStatus::Queued)
        })
        .map(|(id, _)| id.clone())
        .collect();
    let state = gone.iter().fold(state, |s, id| {
        set(s, id, |n| NodeRunState {
            status: NodeStatus::Skipped,
            reason: Some(REMOVED.into()),
            ..n
        })
    });
    let stranded = pick::stranded(graph, &statuses(&state));
    stranded.iter().fold(state, |s, id| {
        set(s, id, |n| NodeRunState {
            status: NodeStatus::Skipped,
            reason: Some(STRANDED.into()),
            ..n
        })
    })
}

fn blocked_of(dir: &Path, state: &RunState) -> Vec<AgentBlocked> {
    state
        .nodes
        .iter()
        .filter(|(_, n)| n.status == NodeStatus::Waiting)
        .map(|(id, n)| {
            let asked = questions::read(&node_run_dir(dir, &state.run_id, id))
                .into_iter()
                .rev()
                .find(|q| q.answer.is_none())
                .map(|q| q.question);
            AgentBlocked {
                node_id: id.clone(),
                question: asked.or_else(|| n.reason.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

/// The run must still be open for new work.
pub fn require_running(run: &AgentRun) -> AppResult<()> {
    match run.state.status {
        RunStatus::Running => Ok(()),
        RunStatus::Cancelled => Err(AppError::new(
            "run_cancelled",
            format!(
                "run {} was stopped (in the app, or by a newer graph_open); call graph_open to \
                 start a new run",
                run.state.run_id
            ),
        )),
        RunStatus::Succeeded | RunStatus::Failed => Err(AppError::new(
            "run_finished",
            format!(
                "run {} is finished ({}); call graph_open to start a new run",
                run.state.run_id,
                super::run_word(run.state.status)
            ),
        )),
    }
}

/// `graph_next`: up to `max` ready nodes (marked `running`, briefs written),
/// or what the run waits on, or `done` with the summary.
pub fn next(
    dir: &Path,
    run_id: &str,
    caller: &Caller,
    max: usize,
    retry_id: Option<&str>,
) -> AppResult<AgentNext> {
    let graph = read_graph(dir)?;
    let _lock = run::lock_run(dir, run_id)?;
    let run = run::read_agent(dir, run_id)?;
    let no_retry = retry_id.map_or(true, |r| r.trim().is_empty());
    if matches!(run.state.status, RunStatus::Succeeded | RunStatus::Failed) && no_retry {
        return Ok(done(&graph, &run));
    }
    let retry_id = retry_id.map(str::trim).filter(|r| !r.is_empty());
    // A retry reopens a run that ended failed.
    let run = if retry_id.is_some() && run.state.status == RunStatus::Failed {
        AgentRun {
            state: RunState {
                status: RunStatus::Running,
                finished_at: None,
                ..run.state
            },
            ..run
        }
    } else {
        run
    };
    require_running(&run)?;
    let state = match retry_id {
        Some(id) => retry(&graph, run.state, id)?,
        None => run.state,
    };
    let state = tidy(&graph, state);
    let max = max.clamp(1, NEXT_MAX);
    let picked = pick::pick(&graph, &statuses(&state), max, retry_id);
    let now = Utc::now();
    let mut state = picked.iter().fold(state, |s, id| {
        set(s, id, |n| NodeRunState {
            status: NodeStatus::Running,
            started_at: Some(now),
            finished_at: None,
            reason: None,
            ..n
        })
    });
    let current = statuses(&state);
    let mut nodes = Vec::new();
    for id in &picked {
        let Some(node) = graph.nodes.iter().find(|n| &n.id == id) else {
            continue;
        };
        let text = write_brief(dir, &graph, run_id, node, &current, caller.channel)?;
        let (brief_path, deliverables_dir) = paths_of(dir, run_id, id);
        let title = if node.title.trim().is_empty() {
            id.clone()
        } else {
            node.title.trim().to_string()
        };
        run::log(
            dir,
            run_id,
            Some(id),
            "handout",
            &format!("handed to {}", caller.client),
        );
        nodes.push(AgentBrief {
            node_id: id.clone(),
            title,
            brief: text,
            brief_path,
            deliverables_dir,
        });
    }
    if let Some(id) = retry_id {
        run::log(dir, run_id, Some(id), "note", "retried");
    }
    state = finish_if_settled(state);
    let run = AgentRun {
        state,
        meta: crate::contract_agent::AgentMeta {
            last_seen: now,
            ..run.meta
        },
    };
    run::write(dir, &run)?;
    if run.state.status != RunStatus::Running {
        let text = format!("finished: {}", super::run_word(run.state.status));
        run::log(dir, run_id, None, "run", &text);
        return Ok(done(&graph, &run));
    }
    let waiting_on = run
        .state
        .nodes
        .iter()
        .filter(|(id, n)| n.status == NodeStatus::Running && !picked.contains(id))
        .map(|(id, _)| id.clone())
        .collect();
    Ok(AgentNext {
        run_id: run_id.to_string(),
        nodes,
        waiting_on,
        blocked: blocked_of(dir, &run.state),
        done: false,
        summary: None,
    })
}

fn done(graph: &Graph, run: &AgentRun) -> AgentNext {
    AgentNext {
        run_id: run.state.run_id.clone(),
        nodes: Vec::new(),
        waiting_on: Vec::new(),
        blocked: Vec::new(),
        done: true,
        summary: Some(status_of(graph, run)),
    }
}

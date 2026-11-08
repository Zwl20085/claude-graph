//! `graph_open`, `graph_status`, `graph_note` and the GUI's Stop: choosing
//! or creating the agent run of a folder, reading it back, and ending it.

use std::path::Path;

use chrono::Utc;

use crate::contract::{
    AppError, AppResult, Graph, NodeRunState, NodeStatus, RunMode, RunState, RunStatus,
};
use crate::contract_agent::{AgentMeta, AgentNodeInfo, AgentOverview, AgentStatus, AgentStatusRow};
use crate::graph_util::upstream_map;
use crate::prompt::display_dir;
use crate::run_store::new_run_id;
use crate::store::read_graph;

use super::pick::{self, Statuses};
use super::run::{self, AgentRun};
use super::{first_line, Caller, NOTE_MAX};

/// Reason of nodes ended by the GUI's Stop button.
pub const STOPPED: &str = "stopped in the app";
/// Reason of nodes ended because the agent opened a new run.
pub const REPLACED: &str = "replaced by a new agent run";

pub fn statuses(state: &RunState) -> Statuses {
    state
        .nodes
        .iter()
        .map(|(id, n)| (id.clone(), n.status))
        .collect()
}

fn meta(caller: &Caller) -> AgentMeta {
    AgentMeta {
        client: caller.client.clone(),
        channel: caller.channel.as_str().into(),
        last_seen: Utc::now(),
    }
}

fn node_goal(n: &crate::contract::Node) -> String {
    let text = if n.goal.trim().is_empty() {
        &n.prompt
    } else {
        &n.goal
    };
    first_line(text, 200)
}

fn title_of(n: &crate::contract::Node) -> String {
    if n.title.trim().is_empty() {
        n.id.clone()
    } else {
        n.title.trim().to_string()
    }
}

/// End every open node of `state` with `reason`; the run becomes `status`.
pub fn end_run(state: RunState, status: RunStatus, reason: &str) -> RunState {
    let now = Utc::now();
    let nodes = state
        .nodes
        .into_iter()
        .map(|(id, n)| {
            let open = matches!(
                n.status,
                NodeStatus::Pending
                    | NodeStatus::Queued
                    | NodeStatus::Running
                    | NodeStatus::Waiting
            );
            let n = if open {
                NodeRunState {
                    status: NodeStatus::Cancelled,
                    reason: Some(reason.to_string()),
                    finished_at: Some(now),
                    ..n
                }
            } else {
                n
            };
            (id, n)
        })
        .collect();
    RunState {
        status,
        finished_at: Some(now),
        nodes,
        ..state
    }
}

/// When every node is settled: the run's final status (`succeeded` when all
/// succeeded) and `finished_at`. Unchanged otherwise.
pub fn finish_if_settled(state: RunState) -> RunState {
    if state.status != RunStatus::Running || !pick::settled(&statuses(&state)) {
        return state;
    }
    let ok = state
        .nodes
        .values()
        .all(|n| n.status == NodeStatus::Succeeded);
    RunState {
        status: if ok {
            RunStatus::Succeeded
        } else {
            RunStatus::Failed
        },
        finished_at: Some(Utc::now()),
        ..state
    }
}

fn new_state(dir: &Path, graph: &Graph, run_id: String) -> RunState {
    RunState {
        run_id,
        graph_dir: dir.display().to_string(),
        started_at: Utc::now(),
        finished_at: None,
        status: RunStatus::Running,
        nodes: graph
            .nodes
            .iter()
            .map(|n| (n.id.clone(), NodeRunState::pending()))
            .collect(),
        mode: RunMode::Terminal,
        sessions: Vec::new(),
    }
}

fn overview(dir: &Path, graph: &Graph, run: &AgentRun, attached: bool) -> AgentOverview {
    let ups = upstream_map(graph);
    let nodes = pick::order(graph)
        .iter()
        .filter_map(|id| graph.nodes.iter().find(|n| &n.id == id))
        .map(|n| AgentNodeInfo {
            id: n.id.clone(),
            title: title_of(n),
            goal: node_goal(n),
            deps: ups
                .get(&n.id)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default(),
            resources: n.resources.clone(),
            deliverables: n
                .deliverables
                .iter()
                .map(|d| d.path.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            status: run
                .state
                .nodes
                .get(&n.id)
                .map_or(NodeStatus::Skipped, |s| s.status),
        })
        .collect();
    AgentOverview {
        dir: display_dir(dir),
        graph: graph.name.clone(),
        run_id: run.state.run_id.clone(),
        attached,
        run_status: run.state.status,
        max_parallel: graph.settings.max_parallel.max(1),
        nodes,
    }
}

/// `graph_open`: attach to the folder's unfinished agent run, or start one
/// (`new_run`: the unfinished one is ended first). Refused while a run
/// started from the app is still `running`.
pub fn open(dir: &Path, caller: &Caller, new_run: bool) -> AppResult<AgentOverview> {
    let graph = read_graph(dir)?;
    let _lock = run::lock_open(dir)?;
    if let Some(app_run) = run::running_app_run(dir) {
        return Err(AppError::new(
            "run_active",
            format!(
                "run {app_run} was started from the Claude Graph app and is still running; \
                 cancel it in the app first (or open the folder in the app once if it was \
                 interrupted)"
            ),
        ));
    }
    if let Some(active) = run::active(dir) {
        let _run_lock = run::lock_run(dir, &active.state.run_id)?;
        if !new_run {
            let run = AgentRun {
                meta: meta(caller),
                ..active
            };
            run::write(dir, &run)?;
            let text = format!("{} attached", caller.client);
            run::log(dir, &run.state.run_id, None, "run", &text);
            return Ok(overview(dir, &graph, &run, true));
        }
        let ended = AgentRun {
            state: end_run(active.state, RunStatus::Cancelled, REPLACED),
            ..active
        };
        run::write(dir, &ended)?;
        run::log(dir, &ended.state.run_id, None, "run", REPLACED);
    }
    let run_id = new_run_id(dir, Utc::now())?;
    let run = AgentRun {
        state: new_state(dir, &graph, run_id),
        meta: meta(caller),
    };
    run::write(dir, &run)?;
    let text = format!("started by {} ({})", caller.client, caller.channel.as_str());
    run::log(dir, &run.state.run_id, None, "run", &text);
    Ok(overview(dir, &graph, &run, false))
}

/// The status table of `run`, rows in dependency order.
pub fn status_of(graph: &Graph, run: &AgentRun) -> AgentStatus {
    let order = pick::order(graph);
    let known = order
        .iter()
        .filter(|id| run.state.nodes.contains_key(*id))
        .cloned();
    let removed = run
        .state
        .nodes
        .keys()
        .filter(|id| !order.contains(id))
        .cloned();
    let rows = known
        .chain(removed)
        .filter_map(|id| {
            let n = run.state.nodes.get(&id)?;
            let title = graph
                .nodes
                .iter()
                .find(|g| g.id == id)
                .map_or_else(|| id.clone(), title_of);
            Some(AgentStatusRow {
                id,
                title,
                status: n.status,
                reason: n.reason.clone(),
            })
        })
        .collect();
    AgentStatus {
        run_id: run.state.run_id.clone(),
        run_status: run.state.status,
        client: run.meta.client.clone(),
        rows,
    }
}

/// `run_id`, else the newest agent run of the folder.
pub fn find(dir: &Path, run_id: Option<&str>) -> AppResult<AgentRun> {
    match run_id.map(str::trim).filter(|r| !r.is_empty()) {
        Some(id) => run::read_agent(dir, id),
        None => run::latest(dir).ok_or_else(|| {
            AppError::new(
                "not_found",
                "no agent run in this folder yet; call graph_open first",
            )
        }),
    }
}

/// `graph_status`.
pub fn status(dir: &Path, run_id: Option<&str>) -> AppResult<AgentStatus> {
    let graph = read_graph(dir)?;
    Ok(status_of(&graph, &find(dir, run_id)?))
}

/// `graph_note`: one line in the run log (node-level when `node_id` is given).
pub fn note(dir: &Path, run_id: Option<&str>, node_id: Option<&str>, text: &str) -> AppResult<()> {
    let run = find(dir, run_id)?;
    let text = text.trim();
    if text.is_empty() {
        return Err(AppError::new("invalid_input", "the note is empty"));
    }
    let node = node_id.map(str::trim).filter(|n| !n.is_empty());
    if let Some(id) = node {
        if !run.state.nodes.contains_key(id) {
            return Err(AppError::new(
                "not_found",
                format!("node {id:?} is not in run {}", run.state.run_id),
            ));
        }
    }
    let cut: String = text.chars().take(NOTE_MAX).collect();
    run::log(dir, &run.state.run_id, node, "note", &cut);
    Ok(())
}

/// The GUI's Stop: the agent run becomes `cancelled`, its open nodes too.
/// A finished run is returned unchanged.
pub fn abandon(dir: &Path, run_id: &str) -> AppResult<AgentRun> {
    let _lock = run::lock_run(dir, run_id)?;
    let run = run::read_agent(dir, run_id)?;
    if run.state.status != RunStatus::Running {
        return Ok(run);
    }
    let ended = AgentRun {
        state: end_run(run.state, RunStatus::Cancelled, STOPPED),
        ..run
    };
    run::write(dir, &ended)?;
    run::log(dir, run_id, None, "run", STOPPED);
    Ok(ended)
}

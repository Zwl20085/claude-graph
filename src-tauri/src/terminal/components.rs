//! Splitting a graph into terminal-mode sessions (pure, unit-tested).
//!
//! A session is one weakly connected component (edges in either direction),
//! cut into maximal runs of consecutive nodes with the same effective harness
//! (node.harness, else settings.harness). Components are ordered by their
//! smallest node position (y, then x, then id); inside a component nodes are
//! sent in topological order (Kahn), ties broken the same way. Sids `s1`,
//! `s2`, … are numbered over the whole graph (components, then their harness
//! runs), so a session keeps its sid whatever the run target.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crate::contract::{
    AppError, AppResult, Graph, Node, NodeStatus, RunState, RunTarget, DEFAULT_HARNESS,
};
use crate::events::truncate_chars;
use crate::graph_util::{downstream_map, upstream_map, validate};
use crate::harness::effective;

/// Longest session title, in chars.
pub const TITLE_MAX: usize = 60;
/// Reason of the nodes of a session that come before its start node.
pub const BEFORE_START: &str = "before start node";

/// One session of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPlan {
    pub sid: String,
    pub title: String,
    /// Every node of the component, in the order they are sent.
    pub node_ids: Vec<String>,
    /// Index in `node_ids` of the first node sent; earlier ones are skipped.
    pub start: usize,
    /// Harness profile id of every node of the session.
    pub harness: String,
    /// Index of the connected component; sessions of one component run one
    /// after the other.
    pub component: usize,
}

impl SessionPlan {
    /// The nodes typed into the session.
    pub fn to_send(&self) -> &[String] {
        &self.node_ids[self.start..]
    }

    /// Nodes skipped because they come before the start node.
    pub fn before_start(&self) -> &[String] {
        &self.node_ids[..self.start]
    }
}

/// Position order: y, then x, then id.
fn by_position(a: &Node, b: &Node) -> Ordering {
    a.position
        .y
        .total_cmp(&b.position.y)
        .then(a.position.x.total_cmp(&b.position.x))
        .then(a.id.cmp(&b.id))
}

fn title_of(node: &Node) -> &str {
    if node.title.trim().is_empty() {
        &node.id
    } else {
        node.title.trim()
    }
}

/// Components of `graph`, each in send order, ordered by smallest position.
pub fn components(graph: &Graph) -> Vec<Vec<String>> {
    let nodes: BTreeMap<&str, &Node> = graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let up = upstream_map(graph);
    let down = downstream_map(graph);
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut sorted: Vec<&Node> = graph.nodes.iter().collect();
    sorted.sort_by(|a, b| by_position(a, b));
    let mut out = Vec::new();
    // Visiting nodes in position order makes the first node of each new
    // component its smallest, so `out` is already in sid order.
    for start in sorted {
        if seen.contains(start.id.as_str()) {
            continue;
        }
        let mut members = BTreeSet::new();
        let mut stack = vec![start.id.as_str()];
        while let Some(id) = stack.pop() {
            if !members.insert(id) {
                continue;
            }
            let neighbours = up.get(id).into_iter().chain(down.get(id)).flatten();
            stack.extend(neighbours.map(String::as_str));
        }
        seen.extend(members.iter().copied());
        out.push(kahn(&members, &nodes, &up, &down));
    }
    out
}

/// Topological order of `members`; ready nodes are taken in position order.
fn kahn(
    members: &BTreeSet<&str>,
    nodes: &BTreeMap<&str, &Node>,
    up: &BTreeMap<String, BTreeSet<String>>,
    down: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    let mut indegree: BTreeMap<&str, usize> = members
        .iter()
        .map(|id| (*id, up.get(*id).map_or(0, BTreeSet::len)))
        .collect();
    let mut order = Vec::with_capacity(members.len());
    loop {
        let next = indegree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(id, _)| *id)
            .min_by(|a, b| by_position(nodes[a], nodes[b]));
        let Some(id) = next else { break };
        indegree.remove(id);
        order.push(id.to_string());
        for t in down.get(id).into_iter().flatten() {
            if let Some(d) = indegree.get_mut(t.as_str()) {
                *d = d.saturating_sub(1);
            }
        }
    }
    order
}

/// Titles of the session's own roots (nodes with no upstream inside it)
/// joined with " + ", plus the harness label in parentheses when it is not
/// the default; at most [`TITLE_MAX`] chars.
fn session_title(graph: &Graph, ids: &[String], harness: &str, labels: &Labels) -> String {
    let up = upstream_map(graph);
    let inside = |u: &String| ids.contains(u);
    let titles: Vec<&str> = ids
        .iter()
        .filter(|id| !up.get(*id).is_some_and(|u| u.iter().any(inside)))
        .filter_map(|id| graph.nodes.iter().find(|n| &n.id == id))
        .map(title_of)
        .collect();
    let joined = titles.join(" + ");
    let suffix = if harness == DEFAULT_HARNESS {
        String::new()
    } else {
        format!(" ({})", labels.get(harness).map_or(harness, String::as_str))
    };
    let room = TITLE_MAX.saturating_sub(suffix.chars().count()).max(1);
    let base = if joined.chars().count() > room {
        truncate_chars(&joined, room - 1)
    } else {
        joined
    };
    format!("{base}{suffix}")
}

/// Harness labels by id (for session titles).
pub type Labels = BTreeMap<String, String>;

/// `ids` (a component in send order) cut into maximal runs of consecutive
/// nodes with the same effective harness.
fn split<'a>(graph: &'a Graph, ids: &[String]) -> Vec<(&'a str, Vec<String>)> {
    let mut out: Vec<(&str, Vec<String>)> = Vec::new();
    for id in ids {
        let Some(node) = graph.nodes.iter().find(|n| &n.id == id) else {
            continue;
        };
        let h = effective(node, &graph.settings);
        match out.last_mut() {
            Some((last, part)) if *last == h => part.push(id.clone()),
            _ => out.push((h, vec![id.clone()])),
        }
    }
    out
}

fn failed_like(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Failed | NodeStatus::Blocked | NodeStatus::Skipped | NodeStatus::Cancelled
    )
}

/// Every session of the graph (sids numbered over components, then over
/// the harness runs inside each component).
fn all_sessions(graph: &Graph, labels: &Labels) -> Vec<SessionPlan> {
    let mut all = Vec::new();
    for (component, ids) in components(graph).into_iter().enumerate() {
        for (harness, part) in split(graph, &ids) {
            all.push(SessionPlan {
                sid: format!("s{}", all.len() + 1),
                title: session_title(graph, &part, harness, labels),
                node_ids: part,
                start: 0,
                harness: harness.to_string(),
                component,
            });
        }
    }
    all
}

/// The sessions of `component` starting at session `at`, node `pos`: earlier
/// sessions of the component keep all their nodes before the start.
fn from_session(all: &[SessionPlan], at: usize, pos: usize) -> Vec<SessionPlan> {
    let component = all[at].component;
    all.iter()
        .enumerate()
        .filter(|(_, p)| p.component == component)
        .map(|(i, p)| {
            let start = match i.cmp(&at) {
                Ordering::Less => p.node_ids.len(),
                Ordering::Equal => pos,
                Ordering::Greater => 0,
            };
            SessionPlan { start, ..p.clone() }
        })
        .collect()
}

/// The sessions a run of `target` uses. `prior_runs` is searched for
/// `FailedOf`. A plan whose nodes all come before the start node sends
/// nothing (it only marks them skipped).
pub fn plan(
    graph: &Graph,
    target: &RunTarget,
    prior_runs: &[RunState],
    labels: &Labels,
) -> AppResult<Vec<SessionPlan>> {
    validate(graph)?;
    let all = all_sessions(graph, labels);
    match target {
        RunTarget::All => Ok(all),
        RunTarget::From(id) => all
            .iter()
            .enumerate()
            .find_map(|(i, p)| {
                let pos = p.node_ids.iter().position(|n| n == id)?;
                Some(from_session(&all, i, pos))
            })
            .ok_or_else(|| AppError::new("not_found", format!("no node {id:?}"))),
        RunTarget::FailedOf(run_id) => {
            let run = prior_runs
                .iter()
                .find(|r| &r.run_id == run_id)
                .ok_or_else(|| AppError::new("not_found", format!("no run {run_id:?}")))?;
            let failed = |id: &String| run.nodes.get(id).is_some_and(|s| failed_like(s.status));
            let mut plans = Vec::new();
            let mut done_components = BTreeSet::new();
            for (i, p) in all.iter().enumerate() {
                if done_components.contains(&p.component) {
                    continue;
                }
                if let Some(pos) = p.node_ids.iter().position(failed) {
                    done_components.insert(p.component);
                    plans.extend(from_session(&all, i, pos));
                }
            }
            if plans.is_empty() {
                return Err(AppError::new(
                    "not_found",
                    format!("run {run_id} has no failed, blocked, skipped or cancelled nodes"),
                ));
            }
            Ok(plans)
        }
    }
}

#[cfg(test)]
#[path = "components_tests.rs"]
mod tests;

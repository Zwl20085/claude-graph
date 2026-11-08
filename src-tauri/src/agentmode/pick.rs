//! Which nodes of an agent run may be handed out now (pure, no I/O).
//!
//! The rules are the headless scheduler's (docs/ARCHITECTURE.md §4) applied
//! to the statuses stored in run.json, so every CLI call or MCP server process
//! can decide from the file alone: a node is ready when it is `pending` and
//! every direct upstream node `succeeded`; at most `max_parallel` nodes are
//! `running`; a tag holds at most its capacity (unknown tags 1); `exclusive`
//! runs alone, and while it waits for the floor to clear nothing of lower
//! priority starts. Priority: longest downstream path first, then id.
//! `waiting` (a blocked report) holds no slot; its dependents stay `pending`.

use std::collections::{BTreeMap, BTreeSet};

use crate::contract::{Graph, NodeStatus};
use crate::graph_util::{downstream_map, reachable, topo_order, upstream_map};
use crate::scheduler::EXCLUSIVE;

/// Node statuses of a run, by id.
pub type Statuses = BTreeMap<String, NodeStatus>;

fn tags_of(graph: &Graph) -> BTreeMap<&str, BTreeSet<&str>> {
    graph
        .nodes
        .iter()
        .map(|n| {
            let tags = n.resources.iter().map(String::as_str).collect();
            (n.id.as_str(), tags)
        })
        .collect()
}

/// Longest downstream path in nodes (a sink has 1).
fn priorities(graph: &Graph) -> BTreeMap<String, usize> {
    let down = downstream_map(graph);
    let order = topo_order(graph).unwrap_or_default();
    let mut prio: BTreeMap<String, usize> = BTreeMap::new();
    for id in order.iter().rev() {
        let below = down
            .get(id)
            .map(|d| d.iter().filter_map(|x| prio.get(x)).max().copied())
            .unwrap_or(None)
            .unwrap_or(0);
        prio.insert(id.clone(), below + 1);
    }
    prio
}

fn ids_with(statuses: &Statuses, wanted: &[NodeStatus]) -> Vec<String> {
    statuses
        .iter()
        .filter(|(_, s)| wanted.contains(s))
        .map(|(id, _)| id.clone())
        .collect()
}

/// `pending` nodes whose direct upstream nodes all `succeeded`, highest
/// priority first. Upstream nodes outside the run count as not succeeded.
pub fn ready(graph: &Graph, statuses: &Statuses) -> Vec<String> {
    let up = upstream_map(graph);
    let prio = priorities(graph);
    let mut ids: Vec<String> = ids_with(statuses, &[NodeStatus::Pending, NodeStatus::Queued])
        .into_iter()
        .filter(|id| up.contains_key(id))
        .filter(|id| {
            up[id]
                .iter()
                .all(|u| statuses.get(u) == Some(&NodeStatus::Succeeded))
        })
        .collect();
    ids.sort_by(|a, b| {
        let pa = prio.get(a).copied().unwrap_or(0);
        let pb = prio.get(b).copied().unwrap_or(0);
        pb.cmp(&pa).then(a.cmp(b))
    });
    ids
}

enum Gate {
    Fits,
    Blocked,
    WaitsForExclusive,
}

fn gate(
    id: &str,
    running: &[String],
    tags: &BTreeMap<&str, BTreeSet<&str>>,
    graph: &Graph,
) -> Gate {
    let max = graph.settings.max_parallel.max(1) as usize;
    let tags_of = |n: &str| tags.get(n).cloned().unwrap_or_default();
    if running.iter().any(|r| tags_of(r).contains(EXCLUSIVE)) || running.len() >= max {
        return Gate::Blocked;
    }
    let mine = tags_of(id);
    if mine.contains(EXCLUSIVE) {
        return if running.is_empty() {
            Gate::Fits
        } else {
            Gate::WaitsForExclusive
        };
    }
    let full = mine.iter().any(|tag| {
        let cap = graph
            .settings
            .resources
            .get(*tag)
            .copied()
            .unwrap_or(1)
            .max(1) as usize;
        running.iter().filter(|r| tags_of(r).contains(tag)).count() >= cap
    });
    if full {
        Gate::Blocked
    } else {
        Gate::Fits
    }
}

/// Up to `max` ready nodes that fit beside the running ones, in hand-out
/// order. `first`: a node to consider before the others (a retry).
pub fn pick(graph: &Graph, statuses: &Statuses, max: usize, first: Option<&str>) -> Vec<String> {
    let tags = tags_of(graph);
    let mut running = ids_with(statuses, &[NodeStatus::Running]);
    let mut candidates = ready(graph, statuses);
    if let Some(f) = first {
        if let Some(pos) = candidates.iter().position(|c| c == f) {
            let id = candidates.remove(pos);
            candidates.insert(0, id);
        }
    }
    let mut picked = Vec::new();
    for id in candidates {
        if picked.len() >= max {
            break;
        }
        match gate(&id, &running, &tags, graph) {
            Gate::Fits => {
                running.push(id.clone());
                picked.push(id);
            }
            Gate::Blocked => {}
            Gate::WaitsForExclusive => break,
        }
    }
    picked
}

/// Transitive dependents of `id` that are still `pending` or `queued` (to
/// mark `skipped` after a failure).
pub fn open_dependents(graph: &Graph, statuses: &Statuses, id: &str) -> Vec<String> {
    reachable(&downstream_map(graph), id)
        .into_iter()
        .filter(|d| {
            matches!(
                statuses.get(d),
                Some(NodeStatus::Pending | NodeStatus::Queued)
            )
        })
        .collect()
}

/// Transitive dependents of `id` that were `skipped` (to reopen on a retry).
pub fn skipped_dependents(graph: &Graph, statuses: &Statuses, id: &str) -> Vec<String> {
    reachable(&downstream_map(graph), id)
        .into_iter()
        .filter(|d| statuses.get(d) == Some(&NodeStatus::Skipped))
        .collect()
}

/// Nothing can change any more: no node is pending, queued, running or waiting.
pub fn settled(statuses: &Statuses) -> bool {
    !statuses.values().any(|s| {
        matches!(
            s,
            NodeStatus::Pending | NodeStatus::Queued | NodeStatus::Running | NodeStatus::Waiting
        )
    })
}

/// `pending` nodes that can never become ready (nothing runs or waits and
/// they are not ready): they become `skipped` so the run can end.
pub fn stranded(graph: &Graph, statuses: &Statuses) -> Vec<String> {
    let busy = statuses
        .values()
        .any(|s| matches!(s, NodeStatus::Running | NodeStatus::Waiting));
    if busy || !ready(graph, statuses).is_empty() {
        return Vec::new();
    }
    ids_with(statuses, &[NodeStatus::Pending, NodeStatus::Queued])
}

/// Run ids in dependency order (ties by position, as the graph lists them).
pub fn order(graph: &Graph) -> Vec<String> {
    topo_order(graph).unwrap_or_else(|_| graph.nodes.iter().map(|n| n.id.clone()).collect())
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;

//! Splitting a graph into terminal-mode sessions (pure, unit-tested).
//!
//! A session is one weakly connected component (edges in either direction).
//! Components are numbered `s1`, `s2`, … in the order of their smallest node
//! position (y, then x, then id); inside a component nodes are sent in
//! topological order (Kahn), ties broken the same way. Sids are numbered over
//! the whole graph, so a component keeps its sid whatever the run target.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crate::contract::{AppError, AppResult, Graph, Node, NodeStatus, RunState, RunTarget};
use crate::events::truncate_chars;
use crate::graph_util::{downstream_map, upstream_map, validate};

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

/// Root titles of a component joined with " + ", at most [`TITLE_MAX`] chars.
fn session_title(graph: &Graph, ids: &[String]) -> String {
    let up = upstream_map(graph);
    let titles: Vec<&str> = ids
        .iter()
        .filter(|id| !up.get(*id).is_some_and(|u| !u.is_empty()))
        .filter_map(|id| graph.nodes.iter().find(|n| &n.id == id))
        .map(title_of)
        .collect();
    let joined = titles.join(" + ");
    if joined.chars().count() > TITLE_MAX {
        truncate_chars(&joined, TITLE_MAX - 1)
    } else {
        joined
    }
}

fn failed_like(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Failed | NodeStatus::Blocked | NodeStatus::Skipped | NodeStatus::Cancelled
    )
}

/// The sessions a run of `target` uses. `prior_runs` is searched for `FailedOf`.
pub fn plan(
    graph: &Graph,
    target: &RunTarget,
    prior_runs: &[RunState],
) -> AppResult<Vec<SessionPlan>> {
    validate(graph)?;
    let all: Vec<SessionPlan> = components(graph)
        .into_iter()
        .enumerate()
        .map(|(i, ids)| SessionPlan {
            sid: format!("s{}", i + 1),
            title: session_title(graph, &ids),
            node_ids: ids,
            start: 0,
        })
        .collect();
    match target {
        RunTarget::All => Ok(all),
        RunTarget::From(id) => all
            .into_iter()
            .find_map(|p| {
                let start = p.node_ids.iter().position(|n| n == id)?;
                Some(vec![SessionPlan { start, ..p }])
            })
            .ok_or_else(|| AppError::new("not_found", format!("no node {id:?}"))),
        RunTarget::FailedOf(run_id) => {
            let run = prior_runs
                .iter()
                .find(|r| &r.run_id == run_id)
                .ok_or_else(|| AppError::new("not_found", format!("no run {run_id:?}")))?;
            let failed = |id: &String| run.nodes.get(id).is_some_and(|s| failed_like(s.status));
            let plans: Vec<SessionPlan> = all
                .into_iter()
                .filter_map(|p| {
                    let start = p.node_ids.iter().position(failed)?;
                    Some(SessionPlan { start, ..p })
                })
                .collect();
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
mod tests {
    use super::*;
    use crate::contract::{NodeRunState, RunMode, RunStatus};
    use serde_json::json;

    /// Nodes `(id, x, y)` and edges `(from, to)`.
    fn graph(nodes: &[(&str, f64, f64)], edges: &[(&str, &str)]) -> Graph {
        serde_json::from_value(json!({
            "version": 1,
            "name": "g",
            "nodes": nodes.iter().map(|(id, x, y)| json!({
                "id": id, "title": format!("T {id}"), "position": {"x": x, "y": y}
            })).collect::<Vec<_>>(),
            "edges": edges.iter().enumerate().map(|(i, (f, t))| json!({
                "id": format!("e{i}"), "from": f, "to": t
            })).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    fn sample() -> Graph {
        // Component A: a -> c, b -> c (two roots); component B: x -> y, above A.
        graph(
            &[
                ("a", 200.0, 100.0),
                ("b", 0.0, 100.0),
                ("c", 0.0, 300.0),
                ("x", 500.0, 0.0),
                ("y", 500.0, 200.0),
                ("z", 900.0, 50.0),
            ],
            &[("a", "c"), ("b", "c"), ("x", "y")],
        )
    }

    #[test]
    fn components_are_ordered_by_position_and_topologically() {
        let comps = components(&sample());
        assert_eq!(
            comps,
            vec![
                vec!["x".to_string(), "y".into()],
                vec!["z".into()],
                vec!["b".into(), "a".into(), "c".into()],
            ]
        );
    }

    #[test]
    fn plan_all_numbers_sessions_and_titles_roots() {
        let plans = plan(&sample(), &RunTarget::All, &[]).unwrap();
        let sids: Vec<&str> = plans.iter().map(|p| p.sid.as_str()).collect();
        assert_eq!(sids, ["s1", "s2", "s3"]);
        assert_eq!(plans[2].title, "T b + T a");
        assert_eq!(plans[2].to_send(), ["b", "a", "c"]);
        assert!(plans[2].before_start().is_empty());
    }

    #[test]
    fn plan_from_node_keeps_one_component_and_skips_earlier_nodes() {
        let plans = plan(&sample(), &RunTarget::From("a".into()), &[]).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].sid, "s3");
        assert_eq!(plans[0].before_start(), ["b"]);
        assert_eq!(plans[0].to_send(), ["a", "c"]);
        let err = plan(&sample(), &RunTarget::From("nope".into()), &[]).unwrap_err();
        assert_eq!(err.code, "not_found");
    }

    #[test]
    fn plan_failed_of_starts_at_the_first_failed_node_per_component() {
        let status = |s| NodeRunState {
            status: s,
            ..NodeRunState::pending()
        };
        let run = RunState {
            run_id: "R1".into(),
            graph_dir: "/g".into(),
            started_at: "2026-11-07T10:00:00Z".parse().unwrap(),
            finished_at: None,
            status: RunStatus::Failed,
            nodes: [
                ("x".to_string(), status(NodeStatus::Succeeded)),
                ("y".to_string(), status(NodeStatus::Succeeded)),
                ("b".to_string(), status(NodeStatus::Succeeded)),
                ("a".to_string(), status(NodeStatus::Failed)),
                ("c".to_string(), status(NodeStatus::Skipped)),
            ]
            .into_iter()
            .collect(),
            mode: RunMode::Terminal,
            sessions: Vec::new(),
        };
        let plans = plan(
            &sample(),
            &RunTarget::FailedOf("R1".into()),
            std::slice::from_ref(&run),
        )
        .unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].sid, "s3");
        assert_eq!(plans[0].to_send(), ["a", "c"]);
        let err = plan(&sample(), &RunTarget::FailedOf("R2".into()), &[run]).unwrap_err();
        assert_eq!(err.code, "not_found");
    }

    #[test]
    fn long_titles_are_cut() {
        let mut g = graph(&[("a", 0.0, 0.0), ("b", 1.0, 0.0)], &[]);
        g.nodes[0].title = "x".repeat(100);
        let plans = plan(&g, &RunTarget::All, &[]).unwrap();
        assert_eq!(plans[0].title.chars().count(), TITLE_MAX);
        assert!(plans[0].title.ends_with('…'));
        assert_eq!(plans[1].title, "T b");
    }
}

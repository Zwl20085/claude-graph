//! Graph validation and topology helpers shared by the store and the scheduler.
//!
//! Rules (docs/graph-format.md, "Loading rules"): version 1, unique node ids that
//! are safe folder names, unique edge ids, edges name existing nodes, no cycles
//! (Kahn), deliverable paths relative and inside the deliverables folder.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path};

use crate::contract::{AppError, AppResult, Graph, GRAPH_VERSION};

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::new("invalid_graph", msg)
}

/// A node id is used as a folder name, so it must be a plain, portable name.
pub fn is_valid_node_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Relative, no `..`, no root/prefix. Empty paths are allowed (ignored when run).
pub fn is_safe_relative_path(p: &str) -> bool {
    Path::new(p)
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Validate a graph; the error message names the first problem found.
pub fn validate(graph: &Graph) -> AppResult<()> {
    if graph.version != GRAPH_VERSION {
        return Err(invalid(format!(
            "unsupported graph version {} (expected {GRAPH_VERSION})",
            graph.version
        )));
    }
    let mut ids = BTreeSet::new();
    for node in &graph.nodes {
        if !is_valid_node_id(&node.id) {
            return Err(invalid(format!(
                "node id {:?} is not a valid folder name (use letters, digits, '-', '_', '.')",
                node.id
            )));
        }
        if !ids.insert(node.id.as_str()) {
            return Err(invalid(format!("duplicate node id {:?}", node.id)));
        }
        if let Some(d) = node
            .deliverables
            .iter()
            .find(|d| !is_safe_relative_path(&d.path))
        {
            return Err(invalid(format!(
                "node {:?}: deliverable path {:?} must be relative to its deliverables folder",
                node.id, d.path
            )));
        }
    }
    let mut edge_ids = BTreeSet::new();
    for edge in &graph.edges {
        if !edge_ids.insert(edge.id.as_str()) {
            return Err(invalid(format!("duplicate edge id {:?}", edge.id)));
        }
        for end in [&edge.from, &edge.to] {
            if !ids.contains(end.as_str()) {
                return Err(invalid(format!(
                    "edge {:?} references unknown node {:?}",
                    edge.id, end
                )));
            }
        }
        if edge.from == edge.to {
            return Err(invalid(format!("edge {:?} is a self-loop", edge.id)));
        }
    }
    topo_order(graph).map(|_| ())
}

fn adjacency(graph: &Graph, reverse: bool) -> BTreeMap<String, BTreeSet<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = graph
        .nodes
        .iter()
        .map(|n| (n.id.clone(), BTreeSet::new()))
        .collect();
    for e in &graph.edges {
        let (key, value) = if reverse {
            (&e.to, &e.from)
        } else {
            (&e.from, &e.to)
        };
        map.entry(key.clone()).or_default().insert(value.clone());
    }
    map
}

/// Direct upstream ids per node (sorted, deduplicated). Every node has an entry.
pub fn upstream_map(graph: &Graph) -> BTreeMap<String, BTreeSet<String>> {
    adjacency(graph, true)
}

/// Direct downstream ids per node (sorted, deduplicated). Every node has an entry.
pub fn downstream_map(graph: &Graph) -> BTreeMap<String, BTreeSet<String>> {
    adjacency(graph, false)
}

/// Kahn's algorithm; ties broken by id so the order is deterministic.
pub fn topo_order(graph: &Graph) -> AppResult<Vec<String>> {
    let down = downstream_map(graph);
    let mut indegree: BTreeMap<&str, usize> = down.keys().map(|k| (k.as_str(), 0)).collect();
    for targets in down.values() {
        for t in targets {
            *indegree.entry(t.as_str()).or_default() += 1;
        }
    }
    let mut ready: BTreeSet<&str> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| *k)
        .collect();
    let mut order = Vec::with_capacity(down.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.to_string());
        for t in &down[id] {
            let d = indegree.get_mut(t.as_str()).expect("edge target is a node");
            *d -= 1;
            if *d == 0 {
                ready.insert(t.as_str());
            }
        }
    }
    if order.len() == down.len() {
        Ok(order)
    } else {
        let stuck: Vec<&str> = indegree
            .iter()
            .filter(|(_, d)| **d > 0)
            .map(|(k, _)| *k)
            .collect();
        Err(invalid(format!(
            "the graph has a cycle through: {}",
            stuck.join(", ")
        )))
    }
}

/// All nodes reachable from `start` along `adjacency` (excluding `start` itself).
pub fn reachable(adjacency: &BTreeMap<String, BTreeSet<String>>, start: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        for next in adjacency.get(id).into_iter().flatten() {
            if seen.insert(next.clone()) {
                queue.push_back(next);
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Deliverable, Edge};
    use serde_json::json;

    fn graph(nodes: &[&str], edges: &[(&str, &str)]) -> Graph {
        serde_json::from_value(json!({
            "version": 1,
            "name": "t",
            "nodes": nodes.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
            "edges": edges.iter().enumerate()
                .map(|(i, (f, t))| json!({ "id": format!("e{i}"), "from": f, "to": t }))
                .collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn accepts_example_and_orders_topologically() {
        let g: Graph =
            serde_json::from_str(include_str!("../../examples/hello-pipeline/graph.json")).unwrap();
        validate(&g).unwrap();
        assert_eq!(topo_order(&g).unwrap(), vec!["survey", "design", "report"]);
    }

    #[test]
    fn rejects_bad_graphs() {
        let cases = [
            graph(&["a", "a"], &[]),
            graph(&["a"], &[("a", "zz")]),
            graph(&["a", "b"], &[("a", "b"), ("b", "a")]),
            graph(&["a"], &[("a", "a")]),
            graph(&["a/b"], &[]),
            graph(&[".."], &[]),
        ];
        for g in cases {
            let err = validate(&g).unwrap_err();
            assert_eq!(err.code, "invalid_graph", "{g:?}");
        }
        let mut dup_edge = graph(&["a", "b"], &[("a", "b")]);
        dup_edge.edges.push(Edge {
            id: "e0".into(),
            from: "a".into(),
            to: "b".into(),
        });
        assert_eq!(validate(&dup_edge).unwrap_err().code, "invalid_graph");
        let mut escaping = graph(&["a"], &[]);
        escaping.nodes[0].deliverables.push(Deliverable {
            path: "../x.md".into(),
            description: String::new(),
        });
        assert_eq!(validate(&escaping).unwrap_err().code, "invalid_graph");
    }

    #[test]
    fn cycle_error_names_the_nodes() {
        let g = graph(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("c", "b")]);
        let err = validate(&g).unwrap_err();
        assert!(err.message.contains("b, c"), "{}", err.message);
    }

    #[test]
    fn reachability() {
        let g = graph(&["a", "b", "c", "d"], &[("a", "b"), ("b", "c")]);
        let down = downstream_map(&g);
        assert_eq!(
            reachable(&down, "a").into_iter().collect::<Vec<_>>(),
            vec!["b", "c"]
        );
        assert!(reachable(&down, "d").is_empty());
    }
}

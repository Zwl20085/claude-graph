//! Graph validation and topology helpers shared by the store and the scheduler.
//!
//! Rules (docs/graph-format.md "Loading rules", ARCHITECTURE.md "Trust and
//! validation"): version 1; node ids match `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`,
//! do not end in `.`, are not a Windows device name (CON, PRN, AUX, NUL, COM1-9,
//! LPT1-9) and are unique ignoring case; unique edge ids; edges name existing
//! nodes; no cycles (Kahn); deliverable paths relative and inside the
//! deliverables folder; `allowed_tools` entries are tool names (optionally with
//! a `(...)` rule) and never start with `-`; `working_dir` is a safe relative
//! path or an absolute local path (never UNC / device); `continue_session_of`
//! names a direct upstream node.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path};
use std::sync::LazyLock;

use regex::Regex;

use crate::contract::{AppError, AppResult, Graph, Node, GRAPH_VERSION};
use crate::paths::is_unc_or_device;

static NODE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$").expect("valid regex"));

static TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(mcp__[A-Za-z0-9_]+(__[A-Za-z0-9_]+)?|[A-Za-z][A-Za-z0-9_]*)(\([^\x00-\x1f"&|<>^%!]*\))?$"#,
    )
    .expect("valid regex")
});

/// Session ids passed to `--resume` (the CLI's UUIDs).
static SESSION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-fA-F-]{36}$").expect("valid regex"));

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::new("invalid_graph", msg)
}

/// Windows device names that cannot be folder names (with or without extension).
fn is_reserved_stem(id: &str) -> bool {
    let stem = id.split('.').next().unwrap_or(id).to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        s if s.len() == 4 && (s.starts_with("COM") || s.starts_with("LPT")) => {
            matches!(s.as_bytes()[3], b'1'..=b'9')
        }
        _ => false,
    }
}

/// A node id is used as a folder name, so it must be a plain, portable name.
/// Run ids are checked with the same rule.
pub fn is_valid_node_id(id: &str) -> bool {
    NODE_ID.is_match(id) && !id.ends_with('.') && !is_reserved_stem(id)
}

/// An `allowed_tools` entry: `Read`, `Bash(python:*)`, `mcp__server__tool`.
pub fn is_valid_tool(entry: &str) -> bool {
    !entry.starts_with('-') && TOOL.is_match(entry)
}

/// A CLI session id safe to pass to `--resume`.
pub fn is_valid_session_id(id: &str) -> bool {
    SESSION_ID.is_match(id)
}

/// Relative, no `..`, no root/prefix/drive, on any OS (both separators are
/// checked). Empty paths are allowed (ignored when run).
pub fn is_safe_relative_path(p: &str) -> bool {
    let drive = p.len() >= 2 && p.as_bytes()[1] == b':';
    !drive
        && !p.starts_with(['/', '\\'])
        && !p.split(['/', '\\']).any(|seg| seg == "..")
        && Path::new(p)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// `working_dir` as given (trimmed), `None` when unset or blank.
pub fn working_dir_of(node: &Node) -> Option<&str> {
    node.working_dir
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
}

/// True when `working_dir` is an absolute path (a trust item).
pub fn is_absolute_working_dir(wd: &str) -> bool {
    Path::new(wd).is_absolute()
}

fn validate_node(node: &Node, upstream: &BTreeSet<String>) -> AppResult<()> {
    let id = &node.id;
    if let Some(d) = node
        .deliverables
        .iter()
        .find(|d| !is_safe_relative_path(d.path.trim()))
    {
        return Err(invalid(format!(
            "node {id:?}: deliverable path {:?} must be relative to its deliverables folder",
            d.path
        )));
    }
    if let Some(t) = node
        .allowed_tools
        .iter()
        .flatten()
        .map(|t| t.trim())
        .find(|t| !t.is_empty() && !is_valid_tool(t))
    {
        return Err(invalid(format!(
            "node {id:?}: allowed tool {t:?} is not a tool name (like Read or Bash(python:*))"
        )));
    }
    if let Some(wd) = working_dir_of(node) {
        if is_unc_or_device(wd) {
            return Err(invalid(format!(
                "node {id:?}: working_dir {wd:?} is a network (UNC) or device path; use a local folder"
            )));
        }
        if !is_safe_relative_path(wd) && !is_absolute_working_dir(wd) {
            return Err(invalid(format!(
                "node {id:?}: working_dir {wd:?} must be a relative path inside the graph folder (no ..) or an absolute path"
            )));
        }
    }
    if let Some(up) = node
        .continue_session_of
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        if !upstream.contains(up) {
            return Err(invalid(format!(
                "node {id:?}: continue_session_of {up:?} must be a direct upstream node"
            )));
        }
    }
    Ok(())
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
    let mut folded = BTreeSet::new();
    for node in &graph.nodes {
        if !is_valid_node_id(&node.id) {
            return Err(invalid(format!(
                "node id {:?} is not a valid folder name (start with a letter or digit, then letters, digits, '-', '_', '.'; at most 128 chars; no trailing '.'; not a Windows device name)",
                node.id
            )));
        }
        if !folded.insert(node.id.to_ascii_lowercase()) {
            return Err(invalid(format!(
                "duplicate node id {:?} (ids are compared ignoring case)",
                node.id
            )));
        }
        ids.insert(node.id.as_str());
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
    let upstream = upstream_map(graph);
    for node in &graph.nodes {
        validate_node(node, &upstream[&node.id])?;
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
    fn node_id_rules() {
        let long_ok = "a".repeat(128);
        let long_bad = "a".repeat(129);
        for ok in [
            "a",
            "A1",
            "survey-2",
            "x_y.z",
            long_ok.as_str(),
            "com0",
            "console",
        ] {
            assert!(is_valid_node_id(ok), "{ok}");
        }
        for bad in [
            "",
            ".",
            "..",
            "-a",
            "_a",
            ".a",
            "a.",
            "a/b",
            "a b",
            "con",
            "CON",
            "Nul.txt",
            "com1",
            "LPT9",
            r"a\b",
            long_bad.as_str(),
        ] {
            assert!(!is_valid_node_id(bad), "{bad}");
        }
        let err = validate(&graph(&["Survey", "survey"], &[])).unwrap_err();
        assert!(err.message.contains("ignoring case"), "{}", err.message);
    }

    #[test]
    fn tool_entries() {
        for ok in [
            "Read",
            "Bash(python:*)",
            "Bash(npm run test:*)",
            "mcp__srv",
            "mcp__srv__tool_1",
            "WebFetch(domain:example.com)",
        ] {
            assert!(is_valid_tool(ok), "{ok}");
        }
        for bad in [
            "-p",
            "--dangerously-skip-permissions",
            "Bash(a & b)",
            "Bash(a|b)",
            r#"Bash("x")"#,
            "Bash(%PATH%)",
            "Bash(x)y",
            "1Read",
            "Read Write",
            "Bash(a\nb)",
        ] {
            assert!(!is_valid_tool(bad), "{bad}");
        }
        let mut g = graph(&["a"], &[]);
        g.nodes[0].allowed_tools = Some(vec!["Read".into(), "--add-dir".into()]);
        assert_eq!(validate(&g).unwrap_err().code, "invalid_graph");
        g.nodes[0].allowed_tools = Some(vec!["Read".into(), " ".into()]);
        validate(&g).unwrap();
    }

    #[test]
    fn working_dir_and_continue_session_rules() {
        let mut g = graph(&["a", "b", "c"], &[("a", "b")]);
        for ok in ["sub", "sub/dir", "./x"] {
            g.nodes[1].working_dir = Some(ok.into());
            validate(&g).unwrap();
        }
        let abs = if cfg!(windows) { r"C:\work" } else { "/work" };
        g.nodes[1].working_dir = Some(abs.into());
        validate(&g).unwrap();
        for bad in ["../x", "a/../../x", r"a\..\x", "C:x", r"\x"] {
            g.nodes[1].working_dir = Some(bad.into());
            assert_eq!(validate(&g).unwrap_err().code, "invalid_graph", "{bad}");
        }
        // UNC and verbatim-UNC working dirs are refused even though they are absolute.
        for unc in [
            r"\\server\share",
            r"\\?\UNC\server\share\x",
            "//server/share",
        ] {
            g.nodes[1].working_dir = Some(unc.into());
            let err = validate(&g).unwrap_err();
            assert_eq!(err.code, "invalid_graph", "{unc}");
            assert!(err.message.contains("UNC"), "{unc}: {}", err.message);
        }
        g.nodes[1].working_dir = None;
        g.nodes[1].continue_session_of = Some("a".into());
        validate(&g).unwrap();
        g.nodes[1].continue_session_of = Some("c".into());
        assert!(validate(&g)
            .unwrap_err()
            .message
            .contains("direct upstream"));
        g.nodes[1].continue_session_of = Some(" ".into());
        validate(&g).unwrap();
    }

    #[test]
    fn session_ids() {
        assert!(is_valid_session_id("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(!is_valid_session_id("--dangerously-skip-permissions"));
        assert!(!is_valid_session_id("sess-1"));
        assert!(!is_valid_session_id(
            "0f8fad5b-d9cb-469f-a165-70867728950e "
        ));
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

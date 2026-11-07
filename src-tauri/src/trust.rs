//! Graph trust (ARCHITECTURE.md "Trust"): a graph folder can come from anyone,
//! so settings that widen what the agent may do need the user's explicit
//! consent before the first run, and again whenever they change.
//!
//! Elevated settings ([`TrustKind`]): a node or the default permission mode of
//! `bypassPermissions` / `dontAsk`, an `allowed_tools` entry containing `Bash`,
//! an absolute `working_dir`. A non-null `settings.claude_bin` is reported but
//! is informational (the backend ignores it), so it never blocks a run.
//!
//! [`fingerprint`] is a sha256 over a canonical JSON of every node's
//! `(id, permission_mode, Bash entries, absolute working_dir)` plus
//! `settings.default_permission_mode`. `trust_graph` stores it per canonical
//! graph folder in `<data_dir>/claude-graph/trusted.json`; the graph is trusted
//! while the stored fingerprint matches.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::contract::{
    AppError, AppResult, Graph, PermissionMode, TrustItem, TrustKind, TrustStatus,
};
use crate::graph_util::{is_absolute_working_dir, working_dir_of};
use crate::store::{read_json, write_json};

pub const TRUST_FILE: &str = "trusted.json";

fn bash_entries(tools: Option<&Vec<String>>) -> Vec<String> {
    tools
        .into_iter()
        .flatten()
        .map(|t| t.trim())
        .filter(|t| t.contains("Bash"))
        .map(str::to_string)
        .collect()
}

fn absolute_wd(wd: Option<&str>) -> Option<String> {
    wd.filter(|w| is_absolute_working_dir(w))
        .map(str::to_string)
}

fn mode_item(node_id: Option<&str>, mode: PermissionMode) -> Option<TrustItem> {
    let (kind, name) = match mode {
        PermissionMode::BypassPermissions => (TrustKind::BypassPermissions, "bypassPermissions"),
        PermissionMode::DontAsk => (TrustKind::DontAsk, "dontAsk"),
        PermissionMode::AcceptEdits
        | PermissionMode::Default
        | PermissionMode::Plan
        | PermissionMode::ClaudeSettings
        | PermissionMode::AskApp
        | PermissionMode::Inherit => return None,
    };
    let detail = match node_id {
        Some(_) => format!("permission mode {name}"),
        None => format!("default permission mode {name}"),
    };
    Some(TrustItem {
        node_id: node_id.map(str::to_string),
        kind,
        detail,
    })
}

/// Every elevated or noteworthy setting of `graph`, graph-wide items first.
pub fn trust_items(graph: &Graph) -> Vec<TrustItem> {
    let settings = mode_item(None, graph.settings.default_permission_mode);
    let claude_bin = graph
        .settings
        .claude_bin
        .as_deref()
        .filter(|b| !b.trim().is_empty())
        .map(|b| TrustItem {
            node_id: None,
            kind: TrustKind::ClaudeBinInGraph,
            detail: format!(
                "graph.json names a claude path ({b}); it is ignored, the app settings decide"
            ),
        });
    let nodes = graph.nodes.iter().flat_map(|n| {
        let id = n.id.as_str();
        let mode = mode_item(Some(id), n.permission_mode);
        let tools = bash_entries(n.allowed_tools.as_ref())
            .into_iter()
            .map(move |t| TrustItem {
                node_id: Some(id.to_string()),
                kind: TrustKind::BashTool,
                detail: format!("allowed tool {t}"),
            });
        let wd = absolute_wd(working_dir_of(n)).map(|w| TrustItem {
            node_id: Some(id.to_string()),
            kind: TrustKind::AbsoluteWorkingDir,
            detail: format!("working directory {w} is outside the graph folder"),
        });
        mode.into_iter().chain(tools).chain(wd)
    });
    settings
        .into_iter()
        .chain(claude_bin)
        .chain(nodes)
        .collect()
}

#[derive(Serialize)]
struct NodePrint {
    id: String,
    permission_mode: PermissionMode,
    bash_tools: Vec<String>,
    working_dir: Option<String>,
}

#[derive(Serialize)]
struct GraphPrint {
    default_permission_mode: PermissionMode,
    nodes: Vec<NodePrint>,
}

/// sha256 (lower-case hex) of the canonical JSON of the elevated fields.
pub fn fingerprint(graph: &Graph) -> String {
    let mut nodes: Vec<NodePrint> = graph
        .nodes
        .iter()
        .map(|n| NodePrint {
            id: n.id.clone(),
            permission_mode: n.permission_mode,
            bash_tools: bash_entries(n.allowed_tools.as_ref()),
            working_dir: absolute_wd(working_dir_of(n)),
        })
        .collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    let print = GraphPrint {
        default_permission_mode: graph.settings.default_permission_mode,
        nodes,
    };
    let json = serde_json::to_vec(&print).unwrap_or_default();
    Sha256::digest(&json)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

type TrustStore = BTreeMap<String, String>;

fn load_store(data_dir: &Path) -> TrustStore {
    let file = data_dir.join(TRUST_FILE);
    if !file.is_file() {
        return TrustStore::new();
    }
    read_json(&file).unwrap_or_else(|e| {
        log::warn!("ignoring unreadable {TRUST_FILE}: {e}");
        TrustStore::new()
    })
}

fn key(graph_dir: &Path) -> String {
    graph_dir.display().to_string()
}

/// Trust status of `graph` stored in canonical folder `graph_dir`.
pub fn status(data_dir: &Path, graph_dir: &Path, graph: &Graph) -> TrustStatus {
    let items = trust_items(graph);
    let needs_consent = items.iter().any(|i| !i.kind.is_informational());
    let trusted =
        !needs_consent || load_store(data_dir).get(&key(graph_dir)) == Some(&fingerprint(graph));
    TrustStatus { trusted, items }
}

/// Record the user's consent to the current elevated settings of `graph`.
pub fn trust(data_dir: &Path, graph_dir: &Path, graph: &Graph) -> AppResult<()> {
    let mut store = load_store(data_dir);
    store.insert(key(graph_dir), fingerprint(graph));
    write_json(&data_dir.join(TRUST_FILE), &store)
}

/// `Err(untrusted_graph)` unless the graph may run.
pub fn require_trusted(data_dir: &Path, graph_dir: &Path, graph: &Graph) -> AppResult<()> {
    let st = status(data_dir, graph_dir, graph);
    if st.trusted {
        return Ok(());
    }
    let what: Vec<String> = st
        .items
        .iter()
        .filter(|i| !i.kind.is_informational())
        .map(|i| match &i.node_id {
            Some(id) => format!("{id}: {}", i.detail),
            None => i.detail.clone(),
        })
        .collect();
    Err(AppError::new(
        "untrusted_graph",
        format!(
            "this graph has elevated settings that need your approval: {}",
            what.join("; ")
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph(nodes: serde_json::Value) -> Graph {
        serde_json::from_value(json!({ "version": 1, "name": "t", "nodes": nodes })).unwrap()
    }

    #[test]
    fn plain_graph_is_trusted_without_consent() {
        let tmp = tempfile::tempdir().unwrap();
        let g = graph(json!([{ "id": "a", "allowed_tools": ["Read", "Write"] }]));
        let st = status(tmp.path(), Path::new("/g"), &g);
        assert!(st.trusted && st.items.is_empty());
        require_trusted(tmp.path(), Path::new("/g"), &g).unwrap();
    }

    #[test]
    fn elevated_items_are_listed_and_need_consent() {
        let tmp = tempfile::tempdir().unwrap();
        let abs = if cfg!(windows) {
            r"C:\elsewhere"
        } else {
            "/elsewhere"
        };
        let mut g = graph(json!([
            { "id": "a", "permission_mode": "bypassPermissions" },
            { "id": "b", "permission_mode": "dontAsk", "allowed_tools": ["Read", "Bash(ls:*)"] },
            { "id": "c", "working_dir": abs },
            { "id": "d", "working_dir": "sub" }
        ]));
        g.settings.default_permission_mode = PermissionMode::DontAsk;
        g.settings.claude_bin = Some("C:/evil/claude.exe".into());
        let st = status(tmp.path(), Path::new("/g"), &g);
        assert!(!st.trusted);
        let kinds: Vec<(Option<&str>, TrustKind)> = st
            .items
            .iter()
            .map(|i| (i.node_id.as_deref(), i.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (None, TrustKind::DontAsk),
                (None, TrustKind::ClaudeBinInGraph),
                (Some("a"), TrustKind::BypassPermissions),
                (Some("b"), TrustKind::DontAsk),
                (Some("b"), TrustKind::BashTool),
                (Some("c"), TrustKind::AbsoluteWorkingDir),
            ]
        );
        let err = require_trusted(tmp.path(), Path::new("/g"), &g).unwrap_err();
        assert_eq!(err.code, "untrusted_graph");

        trust(tmp.path(), Path::new("/g"), &g).unwrap();
        assert!(status(tmp.path(), Path::new("/g"), &g).trusted);
        // Another folder with the same graph is not trusted.
        assert!(!status(tmp.path(), Path::new("/other"), &g).trusted);
        // Unrelated edits keep the trust; elevated edits revoke it.
        g.nodes[3].title = "renamed".into();
        g.name = "new name".into();
        assert!(status(tmp.path(), Path::new("/g"), &g).trusted);
        g.nodes[1].allowed_tools = Some(vec!["Bash(rm:*)".into()]);
        assert!(!status(tmp.path(), Path::new("/g"), &g).trusted);
    }

    #[test]
    fn claude_bin_alone_is_informational() {
        let tmp = tempfile::tempdir().unwrap();
        let mut g = graph(json!([{ "id": "a" }]));
        g.settings.claude_bin = Some("x".into());
        let st = status(tmp.path(), Path::new("/g"), &g);
        assert!(st.trusted);
        assert_eq!(st.items.len(), 1);
        assert_eq!(st.items[0].kind, TrustKind::ClaudeBinInGraph);
    }

    #[test]
    fn fingerprint_is_order_independent_and_hex() {
        let a = graph(json!([{ "id": "a" }, { "id": "b", "allowed_tools": ["Bash"] }]));
        let b = graph(json!([{ "id": "b", "allowed_tools": ["Bash"] }, { "id": "a" }]));
        assert_eq!(fingerprint(&a), fingerprint(&b));
        assert_eq!(fingerprint(&a).len(), 64);
        assert!(fingerprint(&a).chars().all(|c| c.is_ascii_hexdigit()));
    }
}

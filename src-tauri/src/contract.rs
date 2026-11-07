//! IPC contract between the Rust backend and the React frontend.
//!
//! This file mirrors `src/types/contract.ts` one-to-one. It is the frozen
//! public API between the two halves of the app (docs/ARCHITECTURE.md §2, §6).
//! Field names are snake_case on both sides. Enum *values* follow the brief
//! (for example `"acceptEdits"`); field names are never renamed.
//!
//! Optional node and settings fields carry `#[serde(default)]` so that older
//! or hand-written `graph.json` files still load.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Current `graph.json` schema version.
pub const GRAPH_VERSION: u32 = 1;

/// Event name: full `RunState` whenever any node status changes.
pub const EVENT_RUN_STATE: &str = "run:state";
/// Event name: one summarised stream-json line of a running node.
pub const EVENT_NODE_EVENT: &str = "node:event";

// ---------------------------------------------------------------------------
// Graph (graph.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Global concurrency limit.
    pub max_parallel: u32,
    /// Capacity per resource tag. Unknown tags have capacity 1.
    pub resources: BTreeMap<String, u32>,
    pub stop_on_failure: bool,
    pub default_model: Model,
    pub default_permission_mode: PermissionMode,
    /// `None` = find `claude` on PATH; otherwise an explicit path.
    pub claude_bin: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let resources = [("cpu", 1), ("memory", 1), ("gpu", 1)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        Self {
            max_parallel: 2,
            resources,
            stop_on_failure: false,
            default_model: Model::Sonnet,
            default_permission_mode: PermissionMode::AcceptEdits,
            claude_bin: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Model {
    Sonnet,
    Opus,
    Haiku,
    /// Node: use `settings.default_model`.
    #[default]
    Inherit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PermissionMode {
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "bypassPermissions")]
    BypassPermissions,
    /// CLI mode that denies every prompt (docs/CLAUDE_CLI_NOTES.md).
    #[serde(rename = "dontAsk")]
    DontAsk,
    /// Node: use `settings.default_permission_mode`.
    #[serde(rename = "inherit")]
    #[default]
    Inherit,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Deliverable {
    /// Relative to `nodes/<id>/deliverables/`.
    pub path: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Slug, unique within the graph, used as folder name.
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub position: Position,
    /// Free-form task text. A node whose structured fields are all empty is a
    /// "simple" node and its brief is just this prompt (docs/ARCHITECTURE.md §3).
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub scope_in: String,
    #[serde(default)]
    pub scope_out: String,
    #[serde(default)]
    pub already_known: String,
    #[serde(default)]
    pub deliverables: Vec<Deliverable>,
    #[serde(default)]
    pub do_nots: Vec<String>,
    #[serde(default)]
    pub model: Model,
    /// Any of cpu, memory, gpu, exclusive, or free-text tags.
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// `None` = graph dir.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Upstream node id whose session is resumed.
    #[serde(default)]
    pub continue_session_of: Option<String>,
    #[serde(default)]
    pub extra_context: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub to: String,
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

/// Which nodes a run executes.
///
/// JSON: `"all"`, `{"from": "<node-id>"}`, `{"failed_of": "<run-id>"}`.
/// (serde's default externally tagged representation gives exactly these.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTarget {
    All,
    From(String),
    FailedOf(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    Queued,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeRunState {
    pub status: NodeStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub session_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub num_turns: Option<u32>,
    /// Why a node failed / was skipped / was cancelled.
    pub reason: Option<String>,
}

impl NodeRunState {
    pub fn pending() -> Self {
        Self {
            status: NodeStatus::Pending,
            started_at: None,
            finished_at: None,
            session_id: None,
            cost_usd: None,
            num_turns: None,
            reason: None,
        }
    }
}

/// Content of `runs/<run-id>/run.json`, and payload of the `run:state` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub run_id: String,
    pub graph_dir: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    /// Keyed by node id. Only nodes in the run's target set appear.
    pub nodes: BTreeMap<String, NodeRunState>,
}

/// Count of nodes per status in one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusCounts {
    pub pending: u32,
    pub queued: u32,
    pub running: u32,
    pub succeeded: u32,
    pub failed: u32,
    pub skipped: u32,
    pub cancelled: u32,
}

/// One row of `list_runs` (shape not given in the brief; see ARCHITECTURE.md §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub counts: StatusCounts,
    /// Sum of `cost_usd` over nodes that reported one; `None` if none did.
    pub total_cost_usd: Option<f64>,
}

/// Content of `runs/<run-id>/<node-id>/summary.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSummary {
    pub session_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub num_turns: Option<u32>,
    pub is_error: bool,
    pub status: NodeStatus,
}

/// Return value of `read_node_result`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeResult {
    /// Content of result.md; `None` if the node produced none.
    pub result_md: Option<String>,
    /// Content of summary.json; `None` if the node has not finished.
    pub summary: Option<NodeSummary>,
    /// Last raw lines of events.jsonl, oldest first.
    pub events_tail: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeEventKind {
    Text,
    ToolUse,
    ToolResult,
    System,
    Result,
    Stderr,
    ParseError,
    /// A follow-up message from the user was queued or sent to the node.
    UserMessage,
}

/// Payload of the `node:event` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeEvent {
    pub run_id: String,
    pub node_id: String,
    pub kind: NodeEventKind,
    pub text: String,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

/// One line of `runs/<run-id>/<node-id>/messages.jsonl`; returned by `list_messages`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeMessage {
    pub role: MessageRole,
    pub text: String,
    pub ts: DateTime<Utc>,
}

/// Return value of `check_claude`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCheck {
    pub found: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Error returned by every command: `{ code, message }`.
///
/// Codes in use: `not_implemented`, `io`, `json`, `invalid_graph`,
/// `not_found`, `claude_not_found`, `spawn_failed`, `run_active`, `cancelled`, `internal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct AppError {
    pub code: String,
    pub message: String,
}

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn not_implemented(command: &str) -> Self {
        Self::new(
            "not_implemented",
            format!("{command} is not implemented yet"),
        )
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::new("io", e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::new("json", e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const EXAMPLE: &str = include_str!("../../examples/hello-pipeline/graph.json");

    /// Positions are f64, so `80` re-serialises as `80.0`; compare numbers by value.
    fn numbers_as_f64(v: Value) -> Value {
        match v {
            Value::Number(n) => json!(n.as_f64()),
            Value::Array(a) => Value::Array(a.into_iter().map(numbers_as_f64).collect()),
            Value::Object(o) => {
                Value::Object(o.into_iter().map(|(k, v)| (k, numbers_as_f64(v))).collect())
            }
            other => other,
        }
    }

    #[test]
    fn example_graph_round_trips() {
        let graph: Graph = serde_json::from_str(EXAMPLE).expect("example parses");
        assert_eq!(graph.version, GRAPH_VERSION);
        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.edges.len(), 2);
        let design = graph.nodes.iter().find(|n| n.id == "design").unwrap();
        assert_eq!(design.resources, vec!["cpu".to_string()]);
        assert_eq!(design.model, Model::Opus);
        assert_eq!(design.permission_mode, PermissionMode::AcceptEdits);

        let mut original: Value = serde_json::from_str(EXAMPLE).unwrap();
        // Fields added after the example was written load as defaults and are written back.
        for node in original["nodes"].as_array_mut().unwrap() {
            node.as_object_mut()
                .unwrap()
                .entry("prompt")
                .or_insert(json!(""));
        }
        let reserialised = serde_json::to_value(&graph).unwrap();
        assert_eq!(
            numbers_as_f64(original),
            numbers_as_f64(reserialised.clone()),
            "serialisation must be lossless"
        );
        let again: Graph = serde_json::from_value(reserialised).unwrap();
        assert_eq!(graph, again);
    }

    #[test]
    fn minimal_node_loads_with_defaults() {
        let g: Graph = serde_json::from_value(json!({
            "version": 1,
            "name": "old",
            "nodes": [{ "id": "a" }]
        }))
        .unwrap();
        let n = &g.nodes[0];
        assert_eq!(n.model, Model::Inherit);
        assert_eq!(n.permission_mode, PermissionMode::Inherit);
        assert!(n.allowed_tools.is_none());
        assert!(n.deliverables.is_empty());
        assert_eq!(g.settings, Settings::default());
        assert!(g.edges.is_empty());
    }

    #[test]
    fn run_target_json_shapes() {
        let cases = [
            (json!("all"), RunTarget::All),
            (
                json!({ "from": "survey" }),
                RunTarget::From("survey".into()),
            ),
            (
                json!({ "failed_of": "20261107T101500Z" }),
                RunTarget::FailedOf("20261107T101500Z".into()),
            ),
        ];
        for (value, target) in cases {
            let parsed: RunTarget = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(parsed, target);
            assert_eq!(serde_json::to_value(&target).unwrap(), value);
        }
    }

    #[test]
    fn enum_values_match_brief() {
        assert_eq!(json!(NodeStatus::Cancelled), json!("cancelled"));
        assert_eq!(json!(RunStatus::Running), json!("running"));
        assert_eq!(json!(NodeEventKind::ToolUse), json!("tool_use"));
        assert_eq!(json!(NodeEventKind::ParseError), json!("parse_error"));
        assert_eq!(json!(PermissionMode::AcceptEdits), json!("acceptEdits"));
        assert_eq!(
            json!(PermissionMode::BypassPermissions),
            json!("bypassPermissions")
        );
        assert_eq!(json!(Model::Haiku), json!("haiku"));
        assert_eq!(json!(PermissionMode::DontAsk), json!("dontAsk"));
        assert_eq!(json!(NodeEventKind::UserMessage), json!("user_message"));
        assert_eq!(json!(MessageRole::Assistant), json!("assistant"));
    }

    #[test]
    fn run_state_round_trips() {
        let mut nodes = BTreeMap::new();
        nodes.insert("survey".to_string(), NodeRunState::pending());
        let state = RunState {
            run_id: "20261107T101500Z".into(),
            graph_dir: "/tmp/g".into(),
            started_at: "2026-11-07T10:15:00Z".parse().unwrap(),
            finished_at: None,
            status: RunStatus::Running,
            nodes,
        };
        let v = serde_json::to_value(&state).unwrap();
        assert_eq!(v["nodes"]["survey"]["status"], json!("pending"));
        assert_eq!(v["started_at"], json!("2026-11-07T10:15:00Z"));
        assert_eq!(v["finished_at"], Value::Null);
        let back: RunState = serde_json::from_value(v).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn app_error_serialises_as_code_and_message() {
        let e = AppError::not_implemented("open_graph");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({ "code": "not_implemented", "message": "open_graph is not implemented yet" })
        );
    }
}

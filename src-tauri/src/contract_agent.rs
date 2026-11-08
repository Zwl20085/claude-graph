//! Agent-called mode (docs/cli-addon.md, docs/ARCHITECTURE.md "Agent-called
//! mode"): the types the `--mcp-graph` server, the `claude-graph graph …` CLI
//! and the GUI share. Mirrored in `src/types/agent.ts`.
//!
//! The tool and CLI names are a public API: keep them stable.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::contract::{NodeStatus, RunStatus};

/// Event name: an [`AgentRunInfo`] whenever the GUI's watcher sees the agent
/// run of the open folder change (or disappear: `run_id` null).
pub const EVENT_AGENT_RUN: &str = "agent_run";
/// Value of `engine` in the run.json of an agent-driven run.
pub const AGENT_ENGINE: &str = "agent";

/// What the agent learns about one node in `graph_open` / `graph status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentNodeInfo {
    pub id: String,
    pub title: String,
    /// The goal, else the first line of the prompt; one line, ≤ 200 chars.
    pub goal: String,
    /// Direct upstream node ids.
    pub deps: Vec<String>,
    pub resources: Vec<String>,
    /// Required deliverable paths, relative to the node's deliverables folder.
    pub deliverables: Vec<String>,
    pub status: NodeStatus,
}

/// Result of `graph_open`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentOverview {
    /// The graph folder, canonical, `/` separators.
    pub dir: String,
    pub graph: String,
    pub run_id: String,
    /// True when an unfinished agent run of the folder was resumed.
    pub attached: bool,
    pub run_status: RunStatus,
    pub max_parallel: u32,
    /// Nodes in dependency order.
    pub nodes: Vec<AgentNodeInfo>,
}

/// One node handed out by `graph_next`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBrief {
    pub node_id: String,
    pub title: String,
    /// The full brief, ending with how to report back.
    pub brief: String,
    /// `runs/<run>/<node>/prompt.md` (the same text, to re-read later).
    pub brief_path: String,
    pub deliverables_dir: String,
}

/// A node whose agent reported `blocked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBlocked {
    pub node_id: String,
    /// The question, else the reason.
    pub question: String,
}

/// One row of a status table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatusRow {
    pub id: String,
    pub title: String,
    pub status: NodeStatus,
    pub reason: Option<String>,
}

/// Result of `graph_status` (and the summary of a finished run).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatus {
    pub run_id: String,
    pub run_status: RunStatus,
    pub client: String,
    pub rows: Vec<AgentStatusRow>,
}

/// Result of `graph_next`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentNext {
    pub run_id: String,
    /// Handed out now (marked `running`).
    pub nodes: Vec<AgentBrief>,
    /// Running nodes that must be reported before more can start.
    pub waiting_on: Vec<String>,
    /// Blocked nodes (their dependents wait); retry them once answered.
    pub blocked: Vec<AgentBlocked>,
    /// Every node settled; `summary` is set.
    pub done: bool,
    pub summary: Option<AgentStatus>,
}

/// Result of `graph_report`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentReport {
    pub run_id: String,
    pub node_id: String,
    /// `succeeded`, `failed` (also: deliverables missing) or `waiting` (blocked).
    pub status: NodeStatus,
    pub reason: Option<String>,
    /// Nodes ready to be handed out by the next `graph_next`.
    pub ready: Vec<String>,
    /// Every node settled.
    pub done: bool,
}

/// The `agent` object of an agent run's run.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMeta {
    /// Who drives the run, as the client named itself (`Claude Code`, `Codex`, `dsh`, …).
    pub client: String,
    /// `mcp` or `cli`.
    pub channel: String,
    /// Last call from the agent.
    pub last_seen: DateTime<Utc>,
}

/// The agent run of the folder open in the GUI (event [`EVENT_AGENT_RUN`],
/// command `agent_watch`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunInfo {
    pub dir: String,
    pub run_id: String,
    pub client: String,
    pub status: RunStatus,
    /// The run is still `running` (the GUI disables "Run all" and shows Stop).
    pub active: bool,
    pub last_seen: DateTime<Utc>,
    /// Latest run-level note (`graph_note` without a node).
    pub last_note: Option<String>,
}

/// One client's way to register the `--mcp-graph` server (Settings → "Use from your CLI").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentClientConfig {
    /// `claude`, `codex` or `dsh`.
    pub id: String,
    pub name: String,
    /// Text to paste (a command or a config snippet).
    pub snippet: String,
    /// Where "Add" writes, shown before asking; `None` when it runs a command.
    pub config_path: Option<String>,
    /// What "Add" runs or writes, in words, for the confirmation.
    pub add_action: String,
    /// The server is already registered with this client.
    pub installed: bool,
}

/// Content of `runs/<run>/<node>/summary.json` of an agent run. Readable as a
/// [`crate::contract::NodeSummary`] (the extra fields are ignored there).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentNodeSummary {
    pub session: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub status: NodeStatus,
    pub reason: Option<String>,
    pub session_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub num_turns: Option<u32>,
    pub is_error: bool,
    pub permission_denials: Vec<String>,
    pub questions: usize,
    pub waited_for_user: bool,
    /// The agent's own list of files it wrote.
    pub deliverables_written: Vec<String>,
    pub client: String,
}

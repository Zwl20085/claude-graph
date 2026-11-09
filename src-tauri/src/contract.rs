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
/// Id of the default harness profile (Claude Code).
pub const DEFAULT_HARNESS: &str = "claude";

pub use crate::contract_inbox::*;
pub use crate::contract_prompt::*;
pub use crate::harness::profile::{HarnessCheck, HarnessKind, HarnessProfile, StopMode};

/// Event name: full `RunState` whenever any node status changes.
pub const EVENT_RUN_STATE: &str = "run:state";
/// Event name: one summarised stream-json line of a running node.
pub const EVENT_NODE_EVENT: &str = "node:event";
/// Event name: full `AssistantState` after every assistant-mode change.
pub const EVENT_ASSISTANT_UPDATE: &str = "assistant_update";
/// Event name: a chunk of terminal output ([`PtyData`]), sent to one session window only.
pub const EVENT_PTY_DATA: &str = "pty:data";
/// Event name: a [`PermissionPrompt`] waiting for the user ("Ask me in the app").
pub const EVENT_PERMISSION_PROMPT: &str = "permission_prompt";
/// Event name: a prompt was resolved ([`PermissionAnswered`]): by the user,
/// by cancel, or by a remembered scope.
pub const EVENT_PERMISSION_ANSWERED: &str = "permission_answered";

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
    /// Deprecated and ignored: kept so older graph.json files load and round-trip.
    /// The backend never reads, touches or executes this path; the CLI location
    /// lives in the app settings ([`AppSettings::claude_bin`]). A non-null value
    /// is reported as an informational `claude_bin_in_graph` trust item.
    pub claude_bin: Option<String>,
    /// How `start_run` runs the graph. Missing in older graph.json files: terminal.
    pub run_mode: RunMode,
    /// Default harness profile id of the nodes (docs/ARCHITECTURE.md
    /// "Harness profiles"). Missing in older graph.json files: `claude`.
    pub harness: String,
}

/// `terminal`: one interactive Claude Code per connected component, in a
/// window, messages typed for the user (docs/ARCHITECTURE.md "Terminal mode").
/// `headless`: the `claude -p` engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    #[default]
    Terminal,
    Headless,
}

impl RunMode {
    /// Serde default of [`RunState::mode`]: run.json files written before
    /// terminal mode existed are headless runs.
    pub fn headless() -> Self {
        RunMode::Headless
    }
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
            run_mode: RunMode::Terminal,
            harness: DEFAULT_HARNESS.to_string(),
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
    /// Pass no `--permission-mode` flag: the user's own Claude Code settings
    /// decide. Default of new graphs.
    #[serde(rename = "claude_settings")]
    ClaudeSettings,
    /// "Ask me in the app": `--permission-mode default` plus the app's own
    /// permission prompt tool, so each prompt becomes a dialog in the app
    /// (docs/ARCHITECTURE.md "Ask me in the app"). Not elevated.
    #[serde(rename = "ask_app")]
    AskApp,
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
    /// Harness profile id; `None` = inherit `settings.harness`.
    #[serde(default)]
    pub harness: Option<String>,
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
    /// The agent ended with `## Status: BLOCKED` (it stopped and asked something).
    /// Scheduled exactly like `Failed`.
    Blocked,
    Skipped,
    Cancelled,
    /// Terminal mode: the agent asked something (reply question or dialog); not settled.
    Waiting,
}

impl NodeStatus {
    /// Failed or blocked: dependents are skipped and the run counts as failed.
    pub fn is_failure(self) -> bool {
        match self {
            NodeStatus::Failed | NodeStatus::Blocked => true,
            NodeStatus::Pending
            | NodeStatus::Queued
            | NodeStatus::Running
            | NodeStatus::Succeeded
            | NodeStatus::Skipped
            | NodeStatus::Cancelled
            | NodeStatus::Waiting => false,
        }
    }

    /// The node has ended (no turn running or pending).
    pub fn is_terminal(self) -> bool {
        match self {
            NodeStatus::Succeeded
            | NodeStatus::Failed
            | NodeStatus::Blocked
            | NodeStatus::Skipped
            | NodeStatus::Cancelled => true,
            NodeStatus::Pending
            | NodeStatus::Queued
            | NodeStatus::Running
            | NodeStatus::Waiting => false,
        }
    }

    /// The serde name (`"blocked"`, …), as written to GRAPH_LOG.md.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeStatus::Pending => "pending",
            NodeStatus::Queued => "queued",
            NodeStatus::Running => "running",
            NodeStatus::Succeeded => "succeeded",
            NodeStatus::Failed => "failed",
            NodeStatus::Blocked => "blocked",
            NodeStatus::Skipped => "skipped",
            NodeStatus::Cancelled => "cancelled",
            NodeStatus::Waiting => "waiting",
        }
    }
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
    /// Why a node failed / was blocked / was skipped / was cancelled.
    pub reason: Option<String>,
    /// Tools the CLI denied without asking, deduplicated, first-seen order,
    /// accumulated over follow-ups. Missing in older run.json files.
    #[serde(default)]
    pub permission_denials: Vec<String>,
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
            permission_denials: Vec::new(),
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
    /// Missing in older run.json files: headless.
    #[serde(default = "RunMode::headless")]
    pub mode: RunMode,
    /// Terminal mode: one entry per session of the run, by sid. Empty in headless runs.
    #[serde(default)]
    pub sessions: Vec<TerminalSession>,
}

/// Terminal mode: state of one session (one connected component).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Spawned (or waiting for a slot); Claude Code has not reported its start yet.
    Starting,
    /// Claude Code is up and the node messages are being typed.
    Running,
    /// Every node message was handled; the process may still be alive (`pid`).
    Done,
    /// Claude Code did not start, or exited before its nodes were done.
    Failed,
    Cancelled,
}

impl SessionStatus {
    /// The session no longer counts against `max_parallel`.
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            SessionStatus::Done | SessionStatus::Failed | SessionStatus::Cancelled
        )
    }
}

/// Return value of `list_sessions`; entry of [`RunState::sessions`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSession {
    /// `s1`, `s2`, … (order of the component's smallest node position).
    pub sid: String,
    pub title: String,
    /// Nodes of the session in the order they are sent.
    pub node_ids: Vec<String>,
    pub status: SessionStatus,
    /// Process id while the session's process is alive; `None` once it is
    /// gone, and always for chained sessions (one process per message).
    pub pid: Option<u32>,
    /// The session takes messages: its process (pty) or its task (chained)
    /// is alive. Missing in older run.json files: `false`.
    #[serde(default)]
    pub alive: bool,
    /// Node whose message was sent last and has not finished.
    pub current_node: Option<String>,
    /// Harness profile id the session runs. Missing in older run.json files: `claude`.
    #[serde(default = "default_harness")]
    pub harness: String,
    /// Id of the question or dialog the session waits on (`list_session_prompts`).
    #[serde(default)]
    pub pending_prompt: Option<String>,
}

/// Serde default of [`TerminalSession::harness`].
pub fn default_harness() -> String {
    DEFAULT_HARNESS.to_string()
}

/// Payload of the `pty:data` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyData {
    pub run_id: String,
    pub sid: String,
    /// The chunk as UTF-8 (a multi-byte character split across reads is held back).
    pub data: String,
    /// 1-based chunk counter of the session; a window drops chunks with
    /// `seq` ≤ the `seq` of the [`PtyReplay`] it already wrote.
    pub seq: u64,
}

/// Return value of `pty_replay`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyReplay {
    /// The output so far (last 512 KiB), exactly the text of chunks 1..=seq.
    pub text: String,
    /// Number of `pty:data` chunks emitted so far for the session.
    pub seq: u64,
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
    pub blocked: u32,
    pub skipped: u32,
    pub cancelled: u32,
    #[serde(default)]
    pub waiting: u32,
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
    /// Same as [`NodeRunState::permission_denials`]. Missing in older files.
    #[serde(default)]
    pub permission_denials: Vec<String>,
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
    #[serde(default)] // terminal mode: questions.jsonl
    pub questions: Vec<QuestionRecord>,
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
    /// The CLI denied a tool call without asking (`system`/`permission_denied`);
    /// text is the tool name plus a short input summary.
    PermissionDenied,
    /// Terminal mode: Claude Code is waiting for the user in a session window.
    Notification,
    /// "Ask me in the app": a tool call waits for the user; text `<tool>: <summary>`.
    PermissionPrompt,
    /// "Ask me in the app": the prompt was answered; text `allowed (<scope>)` or
    /// `denied (<scope>)`, plus `: <message>` when a deny carries one.
    PermissionAnswered,
    /// Terminal mode: the reply ended with a question; text = the question.
    Question,
    /// Terminal mode: a dialog waits in the window; text = question and options.
    Dialog,
    /// Terminal mode: a question or dialog was answered; text = how.
    Answer,
}

/// One tool call waiting for the user's decision (`list_permission_prompts`,
/// event `permission_prompt`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionPrompt {
    /// uuid.
    pub id: String,
    /// `assistant` for Assistant-tab turns.
    pub run_id: String,
    /// `scheduler` for the assistant.
    pub node_id: String,
    /// For example `Bash`, `WebSearch`, `mcp__x__y`.
    pub tool_name: String,
    /// The tool input as received.
    pub input: serde_json::Value,
    /// One line: the command, path, url or query, else the start of the JSON input.
    pub summary: String,
    /// Raw `suggestions` (permission updates) if the CLI sent any.
    pub suggestions: Vec<serde_json::Value>,
    pub ts: DateTime<Utc>,
}

/// How far an answer reaches: this call only, the rest of this node's run, or
/// this graph from now on (persisted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionScope {
    #[default]
    Once,
    Node,
    Graph,
}

/// Argument of `answer_permission`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionAnswer {
    pub id: String,
    pub allow: bool,
    #[serde(default)]
    pub scope: PermissionScope,
    #[serde(default)]
    pub message: Option<String>,
}

/// Payload of the `permission_answered` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionAnswered {
    pub id: String,
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

/// Return value of `launch_options`: what the command line / env asked for at startup.
/// `autorun` is only true when `open_dir` is set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchOptions {
    pub open_dir: Option<String>,
    pub autorun: bool,
}

/// Per-user app settings (`<data_dir>/claude-graph/settings.json`), not per graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Explicit `claude` executable; `None` = env override, `~/.local/bin`, PATH.
    pub claude_bin: Option<String>,
    /// The Vindictive inbox (docs/INBOX.md). Missing in older files: off.
    pub inbox: InboxSettings,
    /// Start Claude Graph with Windows (tauri-plugin-autostart). Missing: off.
    pub autostart: bool,
}

/// Why a graph needs the user's explicit trust before it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustKind {
    BypassPermissions,
    DontAsk,
    BashTool,
    AbsoluteWorkingDir,
    /// Informational only: `settings.claude_bin` in graph.json is ignored.
    ClaudeBinInGraph,
    /// A node prompt starting with `!`: typed into Claude Code it is a shell
    /// command (bash mode).
    ShellPrompt,
    /// Informational: a node prompt starting with `/` (a slash command).
    SlashPrompt,
    /// The graph folder (or a node's working folder) ships Claude Code
    /// project settings: `.claude/settings*.json` or `.mcp.json`.
    ProjectSettings,
    /// The graph picks a harness profile of this machine with its own args
    /// or env (built-ins without changes are exempt).
    CustomHarness,
}

impl TrustKind {
    /// Informational items never make a graph untrusted on their own.
    pub fn is_informational(self) -> bool {
        matches!(self, TrustKind::ClaudeBinInGraph | TrustKind::SlashPrompt)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustItem {
    /// `None` for graph-wide settings.
    pub node_id: Option<String>,
    pub kind: TrustKind,
    pub detail: String,
}

/// Return value of `graph_trust_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStatus {
    pub trusted: bool,
    pub items: Vec<TrustItem>,
}

// ---------------------------------------------------------------------------
// Assistant mode (graph folder `assistant/`, docs/ARCHITECTURE.md "Assistant mode")
// ---------------------------------------------------------------------------

/// State of the scheduler session of one graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantStatus {
    /// No turn running and the scheduler is not waiting on anything.
    Idle,
    /// A turn is running.
    Working,
    /// The last reply asked at least one `## Question`.
    Waiting,
    /// The last reply contained `## Final` and no question.
    Done,
    /// The last turn errored.
    Failed,
    /// The last turn was cancelled by the user.
    Cancelled,
}

/// Content of `assistant/session.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantSession {
    /// CLI session id, used for `--resume`.
    pub session_id: Option<String>,
    pub status: AssistantStatus,
    /// Completed + running turns (turn folders are numbered from 1).
    pub turns: u32,
    /// As reported by the CLI init event; display only.
    pub model: Option<String>,
    /// sha256 hex of the rendered workflow last sent.
    pub workflow_hash: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl AssistantSession {
    /// The state of a graph that has never talked to a scheduler.
    pub fn empty(now: DateTime<Utc>) -> Self {
        Self {
            session_id: None,
            status: AssistantStatus::Idle,
            turns: 0,
            model: None,
            workflow_hash: None,
            last_error: None,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantRole {
    User,
    Scheduler,
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantKind {
    // role user
    Instruction,
    Answer,
    // role scheduler
    Stage,
    Question,
    Decision,
    Note,
    Final,
    // role app
    System,
    Error,
}

/// One line of `assistant/journal.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// uuid v4.
    pub id: String,
    pub ts: DateTime<Utc>,
    /// Turn the message belongs to (0 for app messages outside a turn).
    pub turn: u32,
    pub role: AssistantRole,
    pub kind: AssistantKind,
    /// Stage name or decision title; `None` otherwise.
    pub title: Option<String>,
    /// Body, trimmed.
    pub text: String,
}

/// Return value of the assistant commands and payload of `assistant_update`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantState {
    pub session: AssistantSession,
    /// The whole journal, in order.
    pub messages: Vec<AssistantMessage>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Error returned by every command: `{ code, message }`.
///
/// Codes in use: `not_implemented`, `io`, `json`, `invalid_graph`, `invalid_input`,
/// `not_found`, `claude_not_found`, `spawn_failed`, `run_active`, `untrusted_graph`,
/// `cancelled`, `internal`, `assistant_busy`.
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
#[path = "contract_tests.rs"]
mod tests;

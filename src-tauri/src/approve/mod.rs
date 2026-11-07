//! "Ask me in the app" (docs/ARCHITECTURE.md "Ask me in the app"): headless
//! `claude -p` turns get `--permission-prompt-tool mcp__claude_graph__approve`
//! and an inline `--mcp-config` that starts this app's own binary in the
//! hidden `--mcp-approve` mode. That stdio MCP server relays every permission
//! request over loopback TCP to the running app, which shows a dialog and
//! sends the user's decision back.
//!
//! - [`wire`]: the request the CLI sends, summaries, match keys, decisions.
//! - [`mcp`]: the hidden `--mcp-approve` stdio server.
//! - [`scopes`]: remembered "always for this graph" answers (`approvals.json`).
//! - [`hub`]: the app side: registrations, pending prompts, answers, scopes.
//! - [`registration`]: one run or assistant turn; its `--mcp-config`.
//! - [`listen`]: the loopback listener and one relay connection.
//! - [`args`]: the CLI flags of an `ask_app` turn and the init-event check.

pub mod args;
pub mod hub;
pub mod listen;
pub mod mcp;
pub mod registration;
pub mod scopes;
pub mod wire;

#[cfg(test)]
mod tests;

pub use hub::Approvals;
pub use registration::Registration;

/// MCP server name in `--mcp-config` and `system/init` `mcp_servers`.
pub const SERVER_NAME: &str = "claude_graph";
/// The one tool of the server.
pub const TOOL_NAME: &str = "approve";
/// `--permission-prompt-tool` value.
pub const PROMPT_TOOL: &str = "mcp__claude_graph__approve";
/// Hidden binary mode: `claude-graph --mcp-approve`.
pub const FLAG_MCP_APPROVE: &str = "--mcp-approve";
/// `127.0.0.1:<port>` of the app's approval listener.
pub const ENV_ADDR: &str = "CLAUDE_GRAPH_APPROVE_ADDR";
/// Per-app-start secret; never logged.
pub const ENV_TOKEN: &str = "CLAUDE_GRAPH_APPROVE_TOKEN";
pub const ENV_RUN_ID: &str = "CLAUDE_GRAPH_RUN_ID";
pub const ENV_NODE_ID: &str = "CLAUDE_GRAPH_NODE_ID";
/// Registration id of the run or assistant turn (tells two graphs apart).
pub const ENV_REG: &str = "CLAUDE_GRAPH_APPROVE_REG";
/// Pending prompts beyond this are denied at once.
pub const MAX_PENDING: usize = 64;
/// Deny message of prompts whose run, node or turn was cancelled.
pub const CANCELLED: &str = "cancelled";

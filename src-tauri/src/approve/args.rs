//! CLI flags of an `ask_app` turn, the `system/init` check of the approval
//! server, and redaction of the token-bearing `--mcp-config` value.

use serde_json::Value;

use super::registration::Registration;
use super::{PROMPT_TOOL, SERVER_NAME};

pub const FLAG_PROMPT_TOOL: &str = "--permission-prompt-tool";
pub const FLAG_MCP_CONFIG: &str = "--mcp-config";
/// Shown in place of the `--mcp-config` value in logs and `spawn:` events.
pub const REDACTED: &str = "<claude-graph approval server config>";
/// Variadic flag that must stay last on the command line.
const VARIADIC: &str = "--allowedTools";

/// `--permission-prompt-tool mcp__claude_graph__approve --mcp-config <json>`.
/// `--permission-mode default` comes from the mode itself
/// ([`crate::runner::permission_flag`]); `--strict-mcp-config` is never added
/// (the user's own servers stay available).
pub fn prompt_tool_args(reg: &Registration, node_id: &str) -> Vec<String> {
    vec![
        FLAG_PROMPT_TOOL.into(),
        PROMPT_TOOL.into(),
        FLAG_MCP_CONFIG.into(),
        reg.mcp_config(node_id),
    ]
}

/// `args` with `extra` inserted before `--allowedTools` (variadic, so it
/// stays last), or appended when there is none.
pub fn with_flags(args: Vec<String>, extra: Vec<String>) -> Vec<String> {
    let at = args
        .iter()
        .position(|a| a == VARIADIC)
        .unwrap_or(args.len());
    let (head, tail) = args.split_at(at);
    head.iter()
        .cloned()
        .chain(extra)
        .chain(tail.iter().cloned())
        .collect()
}

/// Whether a turn's arguments use the app's prompt tool.
pub fn uses_prompt_tool(args: &[String]) -> bool {
    args.windows(2)
        .any(|w| w[0] == FLAG_PROMPT_TOOL && w[1] == PROMPT_TOOL)
}

/// `args` for display: the `--mcp-config` value replaced by [`REDACTED`].
pub fn redact(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut hide = false;
    for a in args {
        out.push(if hide {
            REDACTED.to_string()
        } else {
            a.clone()
        });
        hide = a == FLAG_MCP_CONFIG;
    }
    out
}

/// The init event's MCP fields as the accumulator keeps them:
/// `{"mcp_servers": …, "mcp_server_errors": …}`.
pub fn init_mcp_fields(init: &Value) -> Value {
    serde_json::json!({
        "mcp_servers": init.get("mcp_servers").cloned().unwrap_or(Value::Null),
        "mcp_server_errors": init.get("mcp_server_errors").cloned().unwrap_or(Value::Null),
    })
}

/// `None` when `mcp_servers` lists `claude_graph` as connected; else the
/// node's failure reason with the `mcp_server_errors` text, if any.
pub fn server_problem(fields: &Value) -> Option<String> {
    let connected = fields
        .get("mcp_servers")
        .and_then(Value::as_array)
        .is_some_and(|servers| {
            servers.iter().any(|s| {
                s.get("name").and_then(Value::as_str) == Some(SERVER_NAME)
                    && s.get("status").and_then(Value::as_str) == Some("connected")
            })
        });
    if connected {
        return None;
    }
    let errors = match fields.get("mcp_server_errors") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    let reason = "approval server did not connect".to_string();
    Some(if errors.trim().is_empty() {
        reason
    } else {
        format!("{reason}: {}", super::wire::cut(errors.trim(), 300))
    })
}

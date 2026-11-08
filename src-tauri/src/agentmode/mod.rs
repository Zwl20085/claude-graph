//! Agent-called mode (docs/cli-addon.md): a Claude Code / Codex / dsh
//! session (or a person in a shell) pulls the graph's work one node at a
//! time; the app only schedules and keeps the record. Two entry points, one
//! engine:
//!
//! - `claude-graph --mcp-graph`: stdio MCP server ([`mcp_tools`]) with the
//!   tools `graph_open`, `graph_next`, `graph_report`, `graph_note`,
//!   `graph_status` and the prompt `graph_run`.
//! - `claude-graph graph open|next|report|note|status …` ([`cli`]).
//!
//! State lives only in the graph folder (`runs/<run>/run.json` with
//! `"engine": "agent"`), so every call is a separate read-modify-write under
//! a lock file ([`run`]); the GUI watches the folder ([`watch`]). No model,
//! permission or tool flag is involved: the agent runs under its own settings.

pub mod brief;
pub mod cli;
pub mod gui;
pub mod install;
pub mod mcp_tools;
pub mod next;
pub mod open;
pub mod pick;
pub mod report;
pub mod run;
pub mod text;
pub mod watch;

#[cfg(test)]
pub(crate) mod flow_tests;

/// Hidden binary mode: `claude-graph --mcp-graph`.
pub const FLAG_MCP_GRAPH: &str = "--mcp-graph";
/// First argument of the plain CLI: `claude-graph graph <command> …`.
pub const CLI_WORD: &str = "graph";
/// Longest `summary` accepted (bytes); longer text is cut.
pub const SUMMARY_MAX: usize = 256 * 1024;
/// Longest question or note (chars).
pub const NOTE_MAX: usize = 8000;
/// Most nodes one `graph_next` hands out.
pub const NEXT_MAX: usize = 16;

/// How the agent talks to the app (decides the "Report back" text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Mcp,
    Cli,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Mcp => "mcp",
            Channel::Cli => "cli",
        }
    }
}

/// Who is calling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// Friendly client name (`Claude Code`, `Codex`, `dsh`, …).
    pub client: String,
    pub channel: Channel,
}

/// A friendly name for an MCP `clientInfo.name` (or a `--client` value).
pub fn client_name(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    let name = if lower.contains("claude") {
        "Claude Code"
    } else if lower.contains("codex") {
        "Codex"
    } else if lower.contains("dsh") || lower.contains("deepseek") {
        "dsh"
    } else {
        ""
    };
    if !name.is_empty() {
        return name.to_string();
    }
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_alphanumeric() || " -_.".contains(*c))
        .take(40)
        .collect();
    if cleaned.is_empty() {
        "an agent".to_string()
    } else {
        cleaned
    }
}

/// The serde word of a run status.
pub fn run_word(status: crate::contract::RunStatus) -> &'static str {
    use crate::contract::RunStatus;
    match status {
        RunStatus::Running => "running",
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

/// First line of `text`, at most `max` chars.
pub fn first_line(text: &str, max: usize) -> String {
    let line = text.trim().lines().next().unwrap_or_default().trim();
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line.to_string()
    }
}

/// `text` cut to at most `max` bytes at a char boundary.
pub fn cut_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_names() {
        assert_eq!(client_name("claude-code"), "Claude Code");
        assert_eq!(client_name("codex-mcp-client"), "Codex");
        assert_eq!(client_name("dsh-mcp-client"), "dsh");
        assert_eq!(client_name("my<tool>"), "mytool");
        assert_eq!(client_name("  "), "an agent");
    }

    #[test]
    fn cutting() {
        assert_eq!(first_line("\n abc \nx", 10), "abc");
        assert_eq!(first_line("abcdef", 3), "abc…");
        assert_eq!(cut_bytes("aé", 2), "a");
        assert_eq!(cut_bytes("ab", 5), "ab");
    }
}

//! The deepseek-harness adapter: the DeepSeek Harness CLI `dsh` has no
//! interactive terminal UI (0.2.1-rc.2), so its sessions are *chained
//! headless*: one `dsh --profile <p> --json [--session-id <id>] -` process
//! per message (task on stdin), chained by the session id of the first
//! process's `session` event ([`crate::terminal::chained`]). Verified
//! 2026-11-08 (docs/CLAUDE_CLI_NOTES.md "dsh"):
//!
//! - stdout = one JSON event per line: `session {sessionId, cwd}`, `status
//!   {phase: turn_start|step_start|step_end|turn_end, turn, step, usage?,
//!   reason?}`, `thinking {text}`, `text {text}`, `tool_call {callId, tool,
//!   input}`, `tool_result {callId, status: completed|error, result}`,
//!   `final {text}`, `error {message}`;
//! - exit 0 = done, 1 = error (`error` event, `dsh: <message>` on stderr);
//! - an approval in headless fails closed: the `tool_result` is an error
//!   "... requires approval, but no approval channel is available".

use std::ffi::OsString;
use std::time::Duration;

use serde_json::Value;

use crate::approve::wire::summary;
use crate::harness::adapter::{Behavior, Driver, SpawnInput, StopSignal};
use crate::harness::profile::DEFAULT_DSH_PROFILE;
use crate::terminal::deliver::{Expect, FALLBACK_QUIET};
use crate::terminal::screen::Markers;

/// Session chip / window text of a dsh session.
pub const MODE_LABEL: &str = "dsh (headless, chained)";
/// The tool result text of an approval that headless dsh cannot ask for.
const NO_APPROVAL: &str = "no approval channel";
/// Chars of a tool result shown in the window.
const RESULT_SHOWN: usize = 160;

pub fn behavior() -> Behavior {
    Behavior {
        label: String::new(),
        markers: std::sync::Arc::new(Markers::of(&[], &[])),
        start_hook: false,
        expect: Expect::Submit,
        ready_quiet: FALLBACK_QUIET,
        stop: StopSignal::Quiet(Duration::ZERO),
        types_model: false,
        driver: Driver::Chained,
    }
}

/// `--profile <p> --json` + the profile's args (the per-message
/// `--session-id <id>` and the stdin marker `-` are added by the driver).
pub fn args(input: &SpawnInput<'_>) -> Vec<OsString> {
    let profile = input
        .profile
        .dsh_profile
        .as_deref()
        .unwrap_or(DEFAULT_DSH_PROFILE);
    vec!["--profile".into(), profile.into(), "--json".into()]
}

/// One run event of `dsh --json`.
#[derive(Debug, Clone, PartialEq)]
pub enum DshEvent {
    Session {
        id: String,
    },
    TurnEnd {
        reason: String,
    },
    Thinking(String),
    Text(String),
    ToolCall {
        id: String,
        tool: String,
        input: Value,
    },
    ToolResult {
        id: String,
        ok: bool,
        result: String,
    },
    Final(String),
    Error(String),
    /// Known shape we do not show (step status, usage), or an unknown type.
    Other,
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Parse one stdout line (`None`: not a JSON object).
pub fn parse_line(line: &str) -> Option<DshEvent> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if !v.is_object() {
        return None;
    }
    Some(match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "session" => DshEvent::Session {
            id: s(&v, "sessionId"),
        },
        "status" if v.get("phase").and_then(Value::as_str) == Some("turn_end") => {
            DshEvent::TurnEnd {
                reason: v
                    .pointer("/reason/kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            }
        }
        "thinking" => DshEvent::Thinking(s(&v, "text")),
        "text" => DshEvent::Text(s(&v, "text")),
        "tool_call" => DshEvent::ToolCall {
            id: s(&v, "callId"),
            tool: s(&v, "tool"),
            input: v.get("input").cloned().unwrap_or(Value::Null),
        },
        "tool_result" => DshEvent::ToolResult {
            id: s(&v, "callId"),
            ok: v.get("status").and_then(Value::as_str) == Some("completed"),
            result: match v.get("result") {
                Some(Value::String(r)) => r.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            },
        },
        "final" => DshEvent::Final(s(&v, "text")),
        "error" => DshEvent::Error(s(&v, "message")),
        _ => DshEvent::Other,
    })
}

/// The tool result is an approval headless dsh could not ask for.
pub fn approval_denied(ok: bool, result: &str) -> bool {
    !ok && result.contains(NO_APPROVAL)
}

/// One-line summary of a tool call's input (command, path, query, url, …).
pub fn tool_summary(tool: &str, input: &Value) -> String {
    for key in ["command", "file_path", "path", "query", "url", "pattern"] {
        if let Some(v) = input.get(key).and_then(Value::as_str) {
            return cut(v, 120);
        }
    }
    summary(tool, input)
}

fn cut(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let mut out: String = one.chars().take(max - 1).collect();
    out.push('\u{2026}');
    out
}

const DIM: &str = "\x1b[90m";
const CYAN: &str = "\x1b[36m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// Lines with `\r\n` (the window is a terminal).
fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Agent text without control characters but `\n` and `\t` (no escape
/// sequence from a tool result or reply reaches the window's terminal).
pub fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// The window text of an event (`None`: not shown). `tool_of` names the
/// tool of a call id.
pub fn render(ev: &DshEvent, tool_of: impl Fn(&str) -> Option<String>) -> Option<String> {
    Some(match ev {
        DshEvent::Session { id } => format!("{DIM}session {}{RESET}\r\n", clean(id)),
        DshEvent::Text(t) if !t.trim().is_empty() => {
            format!("\u{25cf} {}\r\n", crlf(&clean(t.trim())))
        }
        DshEvent::ToolCall { tool, input, .. } => {
            format!(
                "{CYAN}\u{23fa} {}({}){RESET}\r\n",
                clean(tool),
                clean(&tool_summary(tool, input))
            )
        }
        DshEvent::ToolResult { id, ok, result } => {
            let tool = clean(&tool_of(id).unwrap_or_default());
            let result = &clean(result);
            if approval_denied(*ok, result) {
                format!(
                    "{YELLOW}  \u{26a0} {tool}: needs approval; headless dsh denies it{RESET}\r\n"
                )
            } else if *ok {
                format!("{DIM}  \u{23bf} {}{RESET}\r\n", cut(result, RESULT_SHOWN))
            } else {
                format!("{RED}  \u{23bf} {}{RESET}\r\n", cut(result, RESULT_SHOWN))
            }
        }
        DshEvent::Error(m) => format!("{RED}error: {}{RESET}\r\n", crlf(&clean(m))),
        DshEvent::TurnEnd { reason } if reason != "completed" => {
            format!("{DIM}turn ended: {}{RESET}\r\n", clean(reason))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines of a real `dsh --profile headless --json` run (0.2.1-rc.2).
    const REAL: [&str; 6] = [
        r#"{"type":"session","sessionId":"session-219e7a8d-669e-4cc7-9420-eb466c7b4ff4","cwd":"C:\\w"}"#,
        r#"{"type":"status","phase":"turn_start","turn":1}"#,
        r#"{"type":"tool_call","callId":"call_00_W","tool":"write","input":{"file_path":"C:\\w\\made.txt","content":"hi"}}"#,
        r#"{"type":"tool_result","callId":"call_00_t","status":"error","result":"Error: sandbox escalation to \"danger-full-access\" requires approval, but no approval channel is available"}"#,
        r#"{"type":"status","phase":"turn_end","turn":1,"reason":{"kind":"completed"}}"#,
        r#"{"type":"final","text":"ok"}"#,
    ];

    #[test]
    fn real_events_parse() {
        let evs: Vec<DshEvent> = REAL.iter().filter_map(|l| parse_line(l)).collect();
        assert_eq!(
            evs[0],
            DshEvent::Session {
                id: "session-219e7a8d-669e-4cc7-9420-eb466c7b4ff4".into()
            }
        );
        assert_eq!(evs[1], DshEvent::Other);
        assert!(matches!(&evs[2], DshEvent::ToolCall { tool, .. } if tool == "write"));
        let DshEvent::ToolResult { ok, result, .. } = &evs[3] else {
            panic!("tool result")
        };
        assert!(approval_denied(*ok, result));
        assert_eq!(
            evs[4],
            DshEvent::TurnEnd {
                reason: "completed".into()
            }
        );
        assert_eq!(evs[5], DshEvent::Final("ok".into()));
        assert_eq!(parse_line("dsh: reasoning"), None);
        assert_eq!(parse_line("[1]"), None);
        assert_eq!(
            parse_line(r#"{"type":"error","message":"session \"x\" does not exist"}"#),
            Some(DshEvent::Error("session \"x\" does not exist".into()))
        );
    }

    #[test]
    fn rendering_is_one_line_per_event() {
        let evs: Vec<DshEvent> = REAL.iter().filter_map(|l| parse_line(l)).collect();
        let tool = |_: &str| Some("write".to_string());
        assert!(render(&evs[2], tool)
            .unwrap()
            .contains("write(C:\\w\\made.txt)"));
        assert!(render(&evs[3], tool).unwrap().contains("needs approval"));
        assert_eq!(render(&evs[4], tool), None);
        assert_eq!(render(&evs[1], tool), None);
        assert_eq!(
            render(&DshEvent::Text("a\nb".into()), tool).unwrap(),
            "\u{25cf} a\r\nb\r\n"
        );
        assert_eq!(
            tool_summary("pwsh", &serde_json::json!({"command": "ls  -la"})),
            "ls -la"
        );
    }
}

//! stream-json parsing (docs/CLAUDE_CLI_NOTES.md) and event emission.
//!
//! [`StreamAccumulator::feed`] takes one raw stdout line, returns the
//! `(kind, text)` summaries to emit as `node:event`, and remembers what the
//! runner needs at the end: session id, cost, turns, duration, `is_error`, and
//! the final assistant text. Unknown types are ignored; malformed lines become
//! `parse_error` events and never fail the run.

use serde_json::Value;

use crate::contract::{
    AssistantState, NodeEvent, NodeEventKind, RunState, EVENT_ASSISTANT_UPDATE, EVENT_NODE_EVENT,
    EVENT_RUN_STATE,
};

const TOOL_INPUT_MAX: usize = 200;
const TOOL_RESULT_MAX: usize = 500;
const BAD_LINE_MAX: usize = 200;

/// Sink for backend → frontend events. Tauri implements it in the app; tests
/// use a recorder.
pub trait Emitter: Send + Sync {
    fn run_state(&self, state: &RunState);
    fn node_event(&self, event: &NodeEvent);
    /// Assistant mode: the whole assistant state of one graph after a change.
    fn assistant_update(&self, state: &AssistantState);
}

/// Emitter that sends through a Tauri app handle.
pub struct TauriEmitter(pub tauri::AppHandle);

impl Emitter for TauriEmitter {
    fn run_state(&self, state: &RunState) {
        use tauri::Emitter as _;
        if let Err(e) = self.0.emit(EVENT_RUN_STATE, state) {
            log::warn!("emit {EVENT_RUN_STATE} failed: {e}");
        }
    }

    fn node_event(&self, event: &NodeEvent) {
        use tauri::Emitter as _;
        if let Err(e) = self.0.emit(EVENT_NODE_EVENT, event) {
            log::warn!("emit {EVENT_NODE_EVENT} failed: {e}");
        }
    }

    fn assistant_update(&self, state: &AssistantState) {
        use tauri::Emitter as _;
        if let Err(e) = self.0.emit(EVENT_ASSISTANT_UPDATE, state) {
            log::warn!("emit {EVENT_ASSISTANT_UPDATE} failed: {e}");
        }
    }
}

/// Fields of the final `result` line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultInfo {
    pub subtype: String,
    pub is_error: bool,
    pub result: Option<String>,
    pub errors: Vec<String>,
    pub session_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub num_turns: Option<u32>,
    pub duration_ms: Option<u64>,
    /// Tool names from `permission_denials` (tools the CLI refused without asking).
    pub permission_denials: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StreamAccumulator {
    session_id: Option<String>,
    /// Model named by the `system`/`init` event.
    model: Option<String>,
    last_assistant_text: Option<String>,
    result: Option<ResultInfo>,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse one stdout line; returns the events to emit (possibly none).
    pub fn feed(&mut self, line: &str) -> Vec<(NodeEventKind, String)> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v @ Value::Object(_)) => v,
            _ => {
                let shown = truncate_chars(line, BAD_LINE_MAX);
                return vec![(
                    NodeEventKind::ParseError,
                    format!("not a JSON object: {shown}"),
                )];
            }
        };
        if let Some(sid) = str_field(&value, "session_id") {
            self.session_id = Some(sid);
        }
        match value.get("type").and_then(Value::as_str) {
            Some("system") => {
                if value.get("subtype").and_then(Value::as_str) == Some("init") {
                    self.model = str_field(&value, "model").or(self.model.take());
                }
                system_event(&value).into_iter().collect()
            }
            Some("assistant") => self.assistant_events(&value),
            Some("user") => user_events(&value),
            Some("result") => {
                let info = parse_result(&value);
                let text = result_text(&info);
                self.result = Some(info);
                vec![(NodeEventKind::Result, text)]
            }
            _ => Vec::new(),
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.result
            .as_ref()
            .and_then(|r| r.session_id.as_deref())
            .or(self.session_id.as_deref())
    }

    pub fn result(&self) -> Option<&ResultInfo> {
        self.result.as_ref()
    }

    /// Model reported by the init event, if any.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// Text blocks of the last top-level assistant message; falls back to `result.result`.
    pub fn final_text(&self) -> Option<String> {
        self.last_assistant_text
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| self.result.as_ref().and_then(|r| r.result.clone()))
    }

    fn assistant_events(&mut self, value: &Value) -> Vec<(NodeEventKind, String)> {
        let blocks = content_blocks(value);
        let top_level = value.get("parent_tool_use_id").map_or(true, Value::is_null);
        if top_level {
            let text: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            self.last_assistant_text = Some(text.join("\n"));
        }
        blocks
            .iter()
            .filter_map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") => b
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| (NodeEventKind::Text, t.to_string())),
                Some("tool_use") => {
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
                    let input = b.get("input").map(Value::to_string).unwrap_or_default();
                    let text = format!("{name} {}", truncate_chars(&input, TOOL_INPUT_MAX));
                    Some((NodeEventKind::ToolUse, text.trim_end().to_string()))
                }
                _ => None,
            })
            .collect()
    }
}

fn content_blocks(value: &Value) -> Vec<Value> {
    value
        .pointer("/message/content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn system_event(value: &Value) -> Option<(NodeEventKind, String)> {
    let subtype = value.get("subtype").and_then(Value::as_str).unwrap_or("");
    let text = match subtype {
        "init" => {
            let model = value.get("model").and_then(Value::as_str).unwrap_or("?");
            let version = value
                .get("claude_code_version")
                .and_then(Value::as_str)
                .unwrap_or("?");
            format!("init: model {model}, claude {version}")
        }
        "api_retry" => {
            let num = |k: &str| value.get(k).map(Value::to_string).unwrap_or("?".into());
            let error = value.get("error").and_then(Value::as_str).unwrap_or("?");
            format!(
                "API retry {}/{} after {} ({}), waiting {} ms",
                num("attempt"),
                num("max_retries"),
                error,
                num("error_status"),
                num("retry_delay_ms")
            )
        }
        "" => return None,
        other => other.to_string(),
    };
    Some((NodeEventKind::System, text))
}

fn user_events(value: &Value) -> Vec<(NodeEventKind, String)> {
    content_blocks(value)
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
        .map(|b| {
            let content = match b.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Some(other) => other.to_string(),
                None => String::new(),
            };
            let prefix = if b.get("is_error").and_then(Value::as_bool) == Some(true) {
                "error: "
            } else {
                ""
            };
            let text = format!("{prefix}{}", truncate_chars(&content, TOOL_RESULT_MAX));
            (NodeEventKind::ToolResult, text)
        })
        .collect()
}

fn parse_result(value: &Value) -> ResultInfo {
    ResultInfo {
        subtype: str_field(value, "subtype").unwrap_or_default(),
        is_error: value
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        result: str_field(value, "result"),
        errors: value
            .get("errors")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|e| e.as_str().map(str::to_string).unwrap_or(e.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        session_id: str_field(value, "session_id"),
        cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
        num_turns: value
            .get("num_turns")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        duration_ms: value.get("duration_ms").and_then(Value::as_u64),
        permission_denials: permission_denials(value),
    }
}

/// Tool names of `permission_denials` entries (`{tool_name, …}` objects, or plain strings).
fn permission_denials(value: &Value) -> Vec<String> {
    value
        .get("permission_denials")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| match d {
                    Value::String(s) => Some(s.clone()),
                    other => str_field(other, "tool_name"),
                })
                .filter(|t| !t.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn result_text(info: &ResultInfo) -> String {
    let mut parts = vec![if info.subtype.is_empty() {
        "result".to_string()
    } else {
        info.subtype.clone()
    }];
    if let Some(t) = info.num_turns {
        parts.push(format!("{t} turns"));
    }
    if let Some(c) = info.cost_usd {
        parts.push(format!("${c:.4}"));
    }
    if let Some(d) = info.duration_ms {
        parts.push(format!("{:.1} s", d as f64 / 1000.0));
    }
    if !info.errors.is_empty() {
        parts.push(info.errors.join("; "));
    }
    parts.join(", ")
}

/// At most `max` chars, cut at a char boundary, with `…` when cut.
pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s-1","cwd":"/w","model":"claude-sonnet","tools":["Read"],"permissionMode":"acceptEdits","claude_code_version":"2.1.5"}"#;
    const ASSISTANT_TOOL: &str = r#"{"type":"assistant","session_id":"s-1","parent_tool_use_id":null,"message":{"role":"assistant","content":[{"type":"text","text":"Reading."},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"a.md"}}],"stop_reason":"tool_use"}}"#;
    const TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"file body","is_error":false}]}}"#;
    const SUBAGENT: &str = r#"{"type":"assistant","session_id":"s-1","parent_tool_use_id":"toolu_9","message":{"role":"assistant","content":[{"type":"text","text":"subagent chatter"}]}}"#;
    const FINAL: &str = r#"{"type":"assistant","session_id":"s-1","parent_tool_use_id":null,"message":{"role":"assistant","content":[{"type":"text","text":"Done.\n## Status: DONE"}],"stop_reason":"end_turn"}}"#;
    const SUCCESS: &str = r#"{"type":"result","subtype":"success","session_id":"s-1","is_error":false,"result":"Done.\n## Status: DONE","stop_reason":"end_turn","num_turns":3,"duration_ms":12345,"duration_api_ms":11000,"total_cost_usd":0.04,"usage":{},"modelUsage":{},"permission_denials":[]}"#;
    const ERROR_RESULT: &str = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"errors":["Reached max turns (5)"],"session_id":"s-2","num_turns":5,"total_cost_usd":0.2}"#;
    const API_RETRY: &str = r#"{"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,"retry_delay_ms":500,"error_status":429,"error":"rate_limit"}"#;

    #[test]
    fn success_sequence() {
        let mut acc = StreamAccumulator::new();
        let init = acc.feed(INIT);
        assert_eq!(init[0].0, NodeEventKind::System);
        assert!(init[0].1.contains("claude-sonnet"));
        assert_eq!(
            acc.feed(ASSISTANT_TOOL),
            vec![
                (NodeEventKind::Text, "Reading.".to_string()),
                (
                    NodeEventKind::ToolUse,
                    r#"Read {"file_path":"a.md"}"#.to_string()
                ),
            ]
        );
        assert_eq!(
            acc.feed(TOOL_RESULT),
            vec![(NodeEventKind::ToolResult, "file body".to_string())]
        );
        acc.feed(FINAL);
        acc.feed(SUBAGENT);
        let result = acc.feed(SUCCESS);
        assert_eq!(result[0].0, NodeEventKind::Result);
        assert_eq!(result[0].1, "success, 3 turns, $0.0400, 12.3 s");
        let info = acc.result().unwrap();
        assert!(!info.is_error);
        assert_eq!(info.num_turns, Some(3));
        assert_eq!(info.duration_ms, Some(12345));
        assert_eq!(info.cost_usd, Some(0.04));
        assert!(info.permission_denials.is_empty());
        assert_eq!(acc.model(), Some("claude-sonnet"));
        assert_eq!(acc.session_id(), Some("s-1"));
        assert_eq!(
            acc.final_text().as_deref(),
            Some("Done.\n## Status: DONE"),
            "subagent text must not replace the final top-level message"
        );
    }

    #[test]
    fn error_result_has_no_result_field() {
        let mut acc = StreamAccumulator::new();
        let ev = acc.feed(ERROR_RESULT);
        assert_eq!(
            ev[0].1,
            "error_max_turns, 5 turns, $0.2000, Reached max turns (5)"
        );
        let info = acc.result().unwrap();
        assert!(info.is_error);
        assert_eq!(info.result, None);
        assert_eq!(info.errors, vec!["Reached max turns (5)"]);
        assert_eq!(acc.session_id(), Some("s-2"));
        assert_eq!(acc.final_text(), None);
    }

    #[test]
    fn permission_denials_give_tool_names() {
        let mut acc = StreamAccumulator::new();
        acc.feed(r#"{"type":"result","subtype":"success","is_error":false,"result":"x","permission_denials":[{"tool_name":"Write","tool_use_id":"t1","tool_input":{}},"Bash",{"tool_use_id":"t3"}]}"#);
        assert_eq!(
            acc.result().unwrap().permission_denials,
            vec!["Write", "Bash"]
        );
        assert_eq!(acc.model(), None);
    }

    #[test]
    fn api_retry_is_a_system_event() {
        let mut acc = StreamAccumulator::new();
        let ev = acc.feed(API_RETRY);
        assert_eq!(
            ev,
            vec![(
                NodeEventKind::System,
                "API retry 1/10 after rate_limit (429), waiting 500 ms".to_string()
            )]
        );
    }

    #[test]
    fn unknown_types_are_ignored() {
        let mut acc = StreamAccumulator::new();
        assert!(acc
            .feed(r#"{"type":"rate_limit_event","session_id":"s-3"}"#)
            .is_empty());
        assert!(acc.feed(r#"{"type":"stream_event","event":{}}"#).is_empty());
        assert!(acc.feed(r#"{"no_type":true}"#).is_empty());
        assert_eq!(
            acc.feed(r#"{"type":"system","subtype":"compact_boundary"}"#),
            vec![(NodeEventKind::System, "compact_boundary".to_string())]
        );
        assert!(acc.feed("   ").is_empty());
        assert_eq!(acc.session_id(), Some("s-3"));
    }

    #[test]
    fn malformed_line_becomes_parse_error() {
        let mut acc = StreamAccumulator::new();
        for bad in ["{not json", "[1,2]", "Waiting for background work…"] {
            let ev = acc.feed(bad);
            assert_eq!(ev.len(), 1);
            assert_eq!(ev[0].0, NodeEventKind::ParseError);
            assert!(ev[0].1.contains(bad.trim()), "{}", ev[0].1);
        }
        assert!(acc.result().is_none());
    }

    #[test]
    fn final_text_falls_back_to_result_when_last_message_is_tool_only() {
        let mut acc = StreamAccumulator::new();
        acc.feed(r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","name":"Write","input":{}}]}}"#);
        acc.feed(SUCCESS);
        assert_eq!(acc.final_text().as_deref(), Some("Done.\n## Status: DONE"));
    }

    #[test]
    fn tool_result_array_content_and_error_flag() {
        let mut acc = StreamAccumulator::new();
        let ev = acc.feed(r#"{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"denied"}],"is_error":true}]}}"#);
        assert_eq!(
            ev,
            vec![(NodeEventKind::ToolResult, "error: denied".into())]
        );
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate_chars("héllo", 2), "hé…");
        assert_eq!(truncate_chars("hi", 2), "hi");
        assert_eq!(truncate_chars("日本語テキスト", 3), "日本語…");
    }
}

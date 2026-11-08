//! Wire formats of the approval relay (docs/CLAUDE_CLI_NOTES.md "Permission
//! prompt tool"): the `tools/call` arguments the CLI sends
//! (`{tool_name, input, tool_use_id}`), the relay line, the decision JSON, and
//! the summary and match key of a call.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// Longest accepted tool name.
const TOOL_NAME_MAX: usize = 256;
/// Summary of a tool without a known field: this many chars of its JSON.
const SUMMARY_JSON_MAX: usize = 120;
/// Any summary is cut to this many chars (the match key is never cut).
const SUMMARY_MAX: usize = 2000;
/// Longest deny message passed to the CLI.
pub const MESSAGE_MAX: usize = 2000;
/// Deny message when the user gave none.
pub const DENIED_BY_USER: &str = "denied by the user in Claude Graph";

/// One line from the `--mcp-approve` server to the app. `Debug` is not
/// derived: the token must never reach a log.
#[derive(Clone, Serialize, Deserialize)]
pub struct RelayRequest {
    pub token: String,
    pub run_id: String,
    pub node_id: String,
    /// Registration id ([`super::ENV_REG`]).
    #[serde(default)]
    pub reg: String,
    /// The `tools/call` arguments as the CLI sent them.
    pub arguments: Value,
}

/// The app's one-line answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayReply {
    pub decision: Value,
}

/// A validated permission request.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub tool_name: String,
    pub input: Value,
    pub suggestions: Vec<Value>,
}

/// Validate the `tools/call` arguments.
pub fn parse_call(arguments: &Value) -> Result<Call, String> {
    let obj = arguments
        .as_object()
        .ok_or("permission request arguments are not an object")?;
    let tool_name = obj
        .get("tool_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or("permission request has no tool_name")?;
    if tool_name.chars().count() > TOOL_NAME_MAX || tool_name.chars().any(char::is_control) {
        return Err("permission request has an invalid tool_name".into());
    }
    let input = obj
        .get("input")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let suggestions = obj
        .get("suggestions")
        .or_else(|| obj.get("permission_suggestions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(Call {
        tool_name: tool_name.to_string(),
        input,
        suggestions,
    })
}

/// The input field that identifies a call of a well-known tool.
fn key_field(tool: &str) -> Option<&'static str> {
    match tool {
        "Bash" => Some("command"),
        "Write" | "Edit" => Some("file_path"),
        "WebSearch" => Some("query"),
        "WebFetch" => Some("url"),
        _ => None,
    }
}

fn field<'a>(tool: &str, input: &'a Value) -> Option<&'a str> {
    key_field(tool)
        .and_then(|f| input.get(f))
        .and_then(Value::as_str)
}

pub fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    }
}

/// Bash → command, Write/Edit → file_path, WebSearch → query, WebFetch → url,
/// else the first 120 chars of the JSON input.
pub fn summary(tool: &str, input: &Value) -> String {
    match field(tool, input) {
        Some(text) => cut(text, SUMMARY_MAX),
        None => cut(&canonical_json(input), SUMMARY_JSON_MAX),
    }
}

/// What "the same call" means for remembered answers: the exact command,
/// path, url or query of a well-known tool, else `sha256:<hex>` of the
/// canonical JSON input.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MatchKey {
    pub tool: String,
    pub key: String,
}

/// Bash input fields that do not change what runs (left out of its key).
const BASH_PROSE: &[&str] = &["command", "description"];

pub fn match_key(tool: &str, input: &Value) -> MatchKey {
    let key = match field(tool, input) {
        // Bash: the command plus every other field that changes the call
        // (timeout, background, sandbox), so an approval of `x` never covers
        // `x` run with the sandbox off.
        Some(text) if tool == "Bash" => {
            let rest: serde_json::Map<String, Value> = input
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter(|(k, _)| !BASH_PROSE.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                })
                .unwrap_or_default();
            if rest.is_empty() {
                text.to_string()
            } else {
                format!("{text}\u{1f}{}", canonical_json(&Value::Object(rest)))
            }
        }
        Some(text) => text.to_string(),
        None => format!("sha256:{}", sha256_hex(&canonical_json(input))),
    };
    MatchKey {
        tool: tool.to_string(),
        key,
    }
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// JSON with object keys sorted at every level, written by hand so the
/// result does not depend on serde_json's map-ordering feature.
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical_json(&map[k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

/// `{"behavior":"allow","updatedInput":<input>}` (updatedInput is required).
pub fn allow(input: &Value) -> Value {
    json!({ "behavior": "allow", "updatedInput": input })
}

/// `{"behavior":"deny","message":…}`; an empty message becomes [`DENIED_BY_USER`].
pub fn deny(message: &str) -> Value {
    let message = message.trim();
    let message = if message.is_empty() {
        DENIED_BY_USER.to_string()
    } else {
        cut(message, MESSAGE_MAX)
    };
    json!({ "behavior": "deny", "message": message })
}

/// Constant-time comparison of two tokens.
pub fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

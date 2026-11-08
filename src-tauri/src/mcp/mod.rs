//! Shared plumbing of the app's stdio MCP servers (`--mcp-approve` and
//! `--mcp-graph`): one JSON-RPC object per line, protocol version
//! negotiation, replies, errors and locked line writes.

use std::io::Write;
use std::sync::Mutex;

use serde_json::{json, Value};

/// Protocol versions echoed back; anything else gets [`FALLBACK_VERSION`].
pub const KNOWN_VERSIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
pub const FALLBACK_VERSION: &str = "2025-06-18";
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// A JSON-RPC request read from one line.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: String,
    pub id: Value,
    pub params: Option<Value>,
}

/// `Some(request)` for a request with a non-null id; `None` for
/// notifications and unparsable lines (nothing is answered).
pub fn parse(line: &str) -> Option<Request> {
    let msg = serde_json::from_str::<Value>(line.trim()).ok()?;
    let id = msg.get("id").filter(|id| !id.is_null()).cloned()?;
    Some(Request {
        method: msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        id,
        params: msg.get("params").cloned(),
    })
}

pub fn reply(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The version to answer `initialize` with.
pub fn negotiate(params: Option<&Value>) -> &'static str {
    let asked = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    KNOWN_VERSIONS
        .iter()
        .find(|v| **v == asked)
        .copied()
        .unwrap_or(FALLBACK_VERSION)
}

/// The `initialize` result: `capabilities` as given, server info, and the
/// optional `instructions` text.
pub fn initialize(
    id: &Value,
    params: Option<&Value>,
    capabilities: Value,
    instructions: Option<&str>,
) -> Value {
    let mut result = json!({
        "protocolVersion": negotiate(params),
        "capabilities": capabilities,
        "serverInfo": { "name": "claude-graph", "version": env!("CARGO_PKG_VERSION") },
    });
    if let (Some(text), Some(obj)) = (instructions, result.as_object_mut()) {
        obj.insert("instructions".into(), Value::String(text.to_string()));
    }
    reply(id, result)
}

/// A `tools/call` result with one text block.
pub fn text_result(id: &Value, text: &str, is_error: bool) -> Value {
    reply(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
    )
}

/// Write `msg` as one line and flush; failures go to stderr with `label`.
pub fn write_line<W: Write>(out: &Mutex<W>, msg: &Value, label: &str) {
    let mut out = out.lock().unwrap_or_else(|p| p.into_inner());
    let written = writeln!(out, "{msg}").and_then(|_| out.flush());
    if let Err(e) = written {
        eprintln!("claude-graph {label}: writing stdout failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_notifications_and_garbage() {
        assert_eq!(parse("not json"), None);
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            None
        );
        assert_eq!(parse(r#"{"jsonrpc":"2.0","id":null,"method":"x"}"#), None);
        let r = parse(r#"{"jsonrpc":"2.0","id":"a","method":"ping"}"#).unwrap();
        assert_eq!((r.method.as_str(), r.id), ("ping", json!("a")));
    }

    #[test]
    fn initialize_echoes_known_versions_and_carries_instructions() {
        let p = json!({"protocolVersion": "2025-03-26"});
        let r = initialize(&json!(1), Some(&p), json!({"tools": {}}), Some("do x"));
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["instructions"], "do x");
        let r = initialize(&json!(1), None, json!({}), None);
        assert_eq!(r["result"]["protocolVersion"], FALLBACK_VERSION);
        assert!(r["result"].get("instructions").is_none());
    }
}

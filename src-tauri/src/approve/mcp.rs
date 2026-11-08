//! The hidden `claude-graph --mcp-approve` mode: a minimal MCP server on
//! stdio (one JSON-RPC object per line) with one tool, `approve`. Each
//! `tools/call` is relayed to the app (one JSON line over loopback TCP) and
//! answered with the app's decision as the tool result text. Calls run on
//! their own threads, so `ping` is answered while a dialog is open.
//!
//! Handshake as the real CLI sends it (docs/CLAUDE_CLI_NOTES.md): a
//! `server/discover` probe with a string id (answered method-not-found),
//! `initialize`, `notifications/initialized`, `tools/list`, then `tools/call`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::{json, Value};

use super::wire::{deny, RelayReply, RelayRequest};
use super::{ENV_ADDR, ENV_NODE_ID, ENV_REG, ENV_RUN_ID, ENV_TOKEN, FLAG_MCP_APPROVE, TOOL_NAME};
use crate::mcp::{error, reply, INVALID_PARAMS, METHOD_NOT_FOUND};

/// Longest reply line accepted from the app.
const REPLY_MAX: u64 = 16 * 1024 * 1024;

/// Turns the `tools/call` arguments into a decision (`{behavior, …}`).
pub trait Relay: Send + Sync {
    fn ask(&self, arguments: &Value) -> Value;
}

/// The relay of the real mode: one TCP connection per call.
pub struct TcpRelay {
    pub addr: SocketAddr,
    pub token: String,
    pub run_id: String,
    pub node_id: String,
    pub reg: String,
}

impl TcpRelay {
    /// From the env the app put into `--mcp-config`. The address must be a
    /// loopback one; the token must be non-empty hex.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let addr: SocketAddr = get(ENV_ADDR)
            .ok_or(format!("{ENV_ADDR} is not set"))?
            .trim()
            .parse()
            .map_err(|_| format!("{ENV_ADDR} is not an address"))?;
        if !addr.ip().is_loopback() {
            return Err(format!("{ENV_ADDR} must be a loopback address"));
        }
        let token = get(ENV_TOKEN).unwrap_or_default();
        if token.is_empty() || !token.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("{ENV_TOKEN} is missing or malformed"));
        }
        Ok(Self {
            addr,
            token,
            run_id: get(ENV_RUN_ID).unwrap_or_default(),
            node_id: get(ENV_NODE_ID).unwrap_or_default(),
            reg: get(ENV_REG).unwrap_or_default(),
        })
    }

    fn exchange(&self, arguments: &Value) -> Result<Value, String> {
        let request = RelayRequest {
            token: self.token.clone(),
            run_id: self.run_id.clone(),
            node_id: self.node_id.clone(),
            reg: self.reg.clone(),
            arguments: arguments.clone(),
        };
        let line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
        let mut stream = TcpStream::connect(self.addr).map_err(|e| e.to_string())?;
        stream
            .write_all(format!("{line}\n").as_bytes())
            .map_err(|e| e.to_string())?;
        let mut reply = String::new();
        BufReader::new(stream)
            .take(REPLY_MAX)
            .read_line(&mut reply)
            .map_err(|e| e.to_string())?;
        if reply.trim().is_empty() {
            return Err("the app closed the connection without an answer".into());
        }
        let reply: RelayReply = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
        Ok(reply.decision)
    }
}

impl Relay for TcpRelay {
    /// Never fails: an unreachable app is a deny.
    fn ask(&self, arguments: &Value) -> Value {
        self.exchange(arguments).unwrap_or_else(|e| {
            eprintln!("claude-graph --mcp-approve: {e}");
            deny(&format!("Claude Graph could not be asked: {e}"))
        })
    }
}

fn initialize(id: &Value, params: Option<&Value>) -> Value {
    crate::mcp::initialize(id, params, json!({ "tools": {} }), None)
}

fn tools_list(id: &Value) -> Value {
    reply(
        id,
        json!({ "tools": [{
            "name": TOOL_NAME,
            "description": "Ask the Claude Graph user to allow or deny a tool call.",
            "inputSchema": { "type": "object", "additionalProperties": true },
        }]}),
    )
}

/// What one input line needs.
pub enum Action {
    /// Write this response now.
    Respond(Value),
    /// Relay these `tools/call` arguments, then answer request `id`.
    Call { id: Value, arguments: Value },
    /// Notification or unparsable line: nothing to write.
    Nothing,
}

/// Decide what to do with one line (no I/O).
pub fn handle(line: &str) -> Action {
    let Some(req) = crate::mcp::parse(line) else {
        return Action::Nothing;
    };
    let (method, id, params) = (req.method.as_str(), req.id, req.params.as_ref());
    match method {
        "initialize" => Action::Respond(initialize(&id, params)),
        "ping" => Action::Respond(reply(&id, json!({}))),
        "tools/list" => Action::Respond(tools_list(&id)),
        "tools/call" => {
            let name = params.and_then(|p| p.get("name")).and_then(Value::as_str);
            if name != Some(TOOL_NAME) {
                return Action::Respond(error(&id, INVALID_PARAMS, "unknown tool"));
            }
            let arguments = params
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            Action::Call { id, arguments }
        }
        _ => Action::Respond(error(&id, METHOD_NOT_FOUND, "method not found")),
    }
}

/// The `tools/call` result carrying `decision` as JSON text.
pub fn call_result(id: &Value, decision: &Value) -> Value {
    reply(
        id,
        json!({ "content": [{ "type": "text", "text": decision.to_string() }] }),
    )
}

fn write_line<W: Write>(out: &Mutex<W>, msg: &Value) {
    crate::mcp::write_line(out, msg, FLAG_MCP_APPROVE);
}

/// Serve until `input` ends; returns the threads of calls still being
/// relayed. The real mode exits without waiting for them (closing their
/// connections tells the app the asker is gone); tests join them.
pub fn serve<R, W>(input: R, output: W, relay: Arc<dyn Relay>) -> Vec<JoinHandle<()>>
where
    R: BufRead,
    W: Write + Send + 'static,
{
    let out = Arc::new(Mutex::new(output));
    let mut calls = Vec::new();
    for line in input.lines() {
        let Ok(line) = line else { break };
        match handle(&line) {
            Action::Respond(msg) => write_line(&out, &msg),
            Action::Nothing => {}
            Action::Call { id, arguments } => {
                let (out, relay) = (out.clone(), relay.clone());
                calls.push(std::thread::spawn(move || {
                    let decision = relay.ask(&arguments);
                    write_line(&out, &call_result(&id, &decision));
                }));
            }
        }
    }
    calls
}

/// `Some(exit code)` when argv (after the executable) is exactly
/// `--mcp-approve`; runs the server on this process's stdio.
pub fn from_process() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 1 || args[0] != FLAG_MCP_APPROVE {
        return None;
    }
    let relay = match TcpRelay::from_env(|k| std::env::var(k).ok()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("claude-graph {FLAG_MCP_APPROVE}: {e}");
            return Some(2);
        }
    };
    let stdin = std::io::stdin();
    let pending = serve(stdin.lock(), std::io::stdout(), Arc::new(relay));
    let open = pending.iter().filter(|h| !h.is_finished()).count();
    if open > 0 {
        eprintln!(
            "claude-graph {FLAG_MCP_APPROVE}: stdin closed with {} call(s) unanswered",
            open
        );
    }
    Some(0)
}

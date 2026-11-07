use std::io::{BufRead, BufReader, Cursor, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::args::{redact, server_problem, uses_prompt_tool, with_flags, REDACTED};
use super::hub::{Approvals, Submitted};
use super::mcp::{handle, serve, Action, Relay, TcpRelay};
use super::wire::{canonical_json, match_key, parse_call, summary, RelayRequest};
use super::*;
use crate::contract::{
    AssistantState, NodeEvent, NodeEventKind, PermissionAnswer, PermissionAnswered,
    PermissionPrompt, PermissionScope, RunState,
};
use crate::events::Emitter;

#[derive(Default)]
struct Rec {
    events: Mutex<Vec<NodeEvent>>,
    prompts: Mutex<Vec<PermissionPrompt>>,
    answered: Mutex<Vec<String>>,
}

impl Emitter for Rec {
    fn run_state(&self, _: &RunState) {}
    fn node_event(&self, e: &NodeEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
    fn assistant_update(&self, _: &AssistantState) {}
    fn permission_prompt(&self, p: &PermissionPrompt) {
        self.prompts.lock().unwrap().push(p.clone());
    }
    fn permission_answered(&self, a: &PermissionAnswered) {
        self.answered.lock().unwrap().push(a.id.clone());
    }
}

struct Fixed(Value);
impl Relay for Fixed {
    fn ask(&self, _: &Value) -> Value {
        self.0.clone()
    }
}

/// `Write` into a shared buffer the test can read after `serve`.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);
impl Write for Shared {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bash_args(command: &str) -> Value {
    json!({"tool_name":"Bash","input":{"command":command,"description":"d"},"tool_use_id":"toolu_1"})
}

#[test]
fn mcp_framing_matches_the_recorded_handshake() {
    let input = [
        r#"{"jsonrpc":"2.0","id":"server-discover-probe-1","method":"server/discover","params":{}}"#,
        r#"{"method":"initialize","params":{"protocolVersion":"2025-11-25"},"jsonrpc":"2.0","id":0}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "not json",
        r#"{"method":"tools/list","jsonrpc":"2.0","id":1}"#,
        r#"{"method":"ping","jsonrpc":"2.0","id":7}"#,
        r#"{"method":"tools/call","params":{"name":"approve","arguments":{"tool_name":"Bash","input":{"command":"ls"}}},"jsonrpc":"2.0","id":2}"#,
    ]
    .join("\n");
    let out = Shared::default();
    let decision = json!({"behavior":"allow","updatedInput":{"command":"ls"}});
    for t in serve(
        Cursor::new(input),
        out.clone(),
        Arc::new(Fixed(decision.clone())),
    ) {
        t.join().unwrap();
    }
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 5, "{text}");
    assert_eq!(lines[0]["id"], "server-discover-probe-1");
    assert_eq!(lines[0]["error"]["code"], -32601);
    assert_eq!(lines[1]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(lines[1]["result"]["capabilities"], json!({"tools":{}}));
    assert_eq!(lines[1]["result"]["serverInfo"]["name"], "claude-graph");
    assert_eq!(lines[2]["result"]["tools"][0]["name"], TOOL_NAME);
    assert_eq!(lines[3], json!({"jsonrpc":"2.0","id":7,"result":{}}));
    let call = &lines[4];
    assert_eq!(call["id"], 2);
    let inner: Value =
        serde_json::from_str(call["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(inner, decision);
}

#[test]
fn mcp_unknown_version_tool_and_method() {
    let Action::Respond(r) = handle(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
    ) else {
        panic!("expected a response")
    };
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    let Action::Respond(r) =
        handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"other"}}"#)
    else {
        panic!("expected a response")
    };
    assert_eq!(r["error"]["code"], -32602);
    assert!(matches!(
        handle(r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#),
        Action::Respond(r) if r["error"]["code"] == -32601
    ));
    assert!(matches!(
        handle(r#"{"jsonrpc":"2.0","method":"x"}"#),
        Action::Nothing
    ));
}

#[test]
fn relay_env_is_validated() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    };
    assert!(TcpRelay::from_env(env(&[(ENV_ADDR, "127.0.0.1:5"), (ENV_TOKEN, "ab12")])).is_ok());
    assert!(TcpRelay::from_env(env(&[(ENV_ADDR, "10.0.0.1:5"), (ENV_TOKEN, "ab")])).is_err());
    assert!(TcpRelay::from_env(env(&[(ENV_ADDR, "127.0.0.1:5"), (ENV_TOKEN, "")])).is_err());
    assert!(TcpRelay::from_env(env(&[(ENV_ADDR, "127.0.0.1:5"), (ENV_TOKEN, "x y")])).is_err());
    assert!(TcpRelay::from_env(env(&[(ENV_TOKEN, "ab")])).is_err());
}

#[test]
fn summaries_and_match_keys() {
    let bash = json!({"command":"echo hi","description":"x"});
    assert_eq!(summary("Bash", &bash), "echo hi");
    let other = json!({"command":"echo hi","description":"other words"});
    assert_eq!(match_key("Bash", &bash), match_key("Bash", &other));
    assert_eq!(
        match_key("Write", &json!({"file_path":"/a","content":"1"})).key,
        "/a"
    );
    assert_eq!(summary("WebSearch", &json!({"query":"q"})), "q");
    assert_eq!(
        summary("WebFetch", &json!({"url":"https://x","prompt":"p"})),
        "https://x"
    );
    let a = json!({"b":1,"a":{"d":[1,{"z":0,"y":1}],"c":null}});
    let b = json!({"a":{"c":null,"d":[1,{"y":1,"z":0}]},"b":1});
    assert_eq!(
        canonical_json(&a),
        r#"{"a":{"c":null,"d":[1,{"y":1,"z":0}]},"b":1}"#
    );
    let (ka, kb) = (match_key("mcp__x__y", &a), match_key("mcp__x__y", &b));
    assert_eq!(ka, kb);
    assert!(ka.key.starts_with("sha256:") && ka.key.len() == 7 + 64);
    assert_ne!(ka, match_key("mcp__x__z", &a));
    let long = json!({"k":"v".repeat(500)});
    assert_eq!(summary("Task", &long).chars().count(), 121);
}

#[test]
fn calls_are_validated() {
    assert!(parse_call(&json!([])).is_err());
    assert!(parse_call(&json!({"input":{}})).is_err());
    assert!(parse_call(&json!({"tool_name":"  "})).is_err());
    assert!(parse_call(&json!({"tool_name":"a\nb"})).is_err());
    let c = parse_call(&json!({"tool_name":"Bash","suggestions":[{"type":"addRules"}]})).unwrap();
    assert_eq!(c.input, json!({}));
    assert_eq!(c.suggestions.len(), 1);
}

#[test]
fn args_flags_redaction_and_init_check() {
    let args: Vec<String> = ["-p", "x", "--allowedTools", "Read"]
        .map(String::from)
        .to_vec();
    let extra = vec![args::FLAG_PROMPT_TOOL.into(), PROMPT_TOOL.into()];
    let out = with_flags(args, extra.clone());
    assert_eq!(out[2..4], extra[..]);
    assert_eq!(out[4], "--allowedTools");
    assert!(uses_prompt_tool(&out));
    let shown = redact(&[
        "--mcp-config".into(),
        "{\"token\":\"secret\"}".into(),
        "a".into(),
    ]);
    assert_eq!(shown, vec!["--mcp-config", REDACTED, "a"]);
    let ok = json!({"mcp_servers":[{"name":"claude_graph","status":"connected"}]});
    assert_eq!(server_problem(&ok), None);
    let failed = json!({"mcp_servers":[{"name":"claude_graph","status":"failed"}],
        "mcp_server_errors":"spawn ENOENT"});
    assert_eq!(
        server_problem(&failed).unwrap(),
        "approval server did not connect: spawn ENOENT"
    );
    assert!(server_problem(&json!({"mcp_servers":null})).is_some());
}

fn request(approvals_reg: &str, run: &str, node: &str, args: Value) -> RelayRequest {
    RelayRequest {
        token: String::new(),
        run_id: run.into(),
        node_id: node.into(),
        reg: approvals_reg.into(),
        arguments: args,
    }
}

fn reg_id(config: &str) -> String {
    let v: Value = serde_json::from_str(config).unwrap();
    v["mcpServers"][SERVER_NAME]["env"][ENV_REG]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn pending_cap_scopes_and_close() {
    let tmp = tempfile::tempdir().unwrap();
    let approvals = Approvals::new(tmp.path().to_path_buf());
    let rec = Arc::new(Rec::default());
    let graph = tmp.path().join("g");
    let reg = approvals
        .register("r1", &graph, Path::new("/app"), rec.clone())
        .unwrap();
    let config = reg.mcp_config("n1");
    let rid = reg_id(&config);
    assert!(config.contains(FLAG_MCP_APPROVE) && config.contains("\"n1\""));
    // Unknown registration: denied at once.
    let Submitted::Now(d) = approvals.submit(&request("nope", "r1", "n1", bash_args("x"))) else {
        panic!("expected a deny")
    };
    assert_eq!(d["behavior"], "deny");
    let mut waiting = Vec::new();
    for i in 0..MAX_PENDING {
        match approvals.submit(&request(&rid, "r1", "n1", bash_args(&format!("c{i}")))) {
            Submitted::Wait(_, rx) => waiting.push(rx),
            Submitted::Now(d) => panic!("unexpected {d}"),
        }
    }
    let Submitted::Now(d) = approvals.submit(&request(&rid, "r1", "n1", bash_args("over"))) else {
        panic!("expected the cap")
    };
    assert!(d["message"].as_str().unwrap().contains("too many"));
    assert_eq!(approvals.list().len(), MAX_PENDING);
    assert_eq!(rec.prompts.lock().unwrap().len(), MAX_PENDING);
    // Allow c0 for the node: a later identical ask is auto-answered.
    let first = approvals.list()[0].clone();
    approvals
        .answer(&PermissionAnswer {
            id: first.id.clone(),
            allow: true,
            scope: PermissionScope::Node,
            message: None,
        })
        .unwrap();
    assert_eq!(waiting.remove(0).await.unwrap()["behavior"], "allow");
    let again = approvals.submit(&request(&rid, "r1", "n1", bash_args("c0")));
    assert!(matches!(again, Submitted::Now(d) if d["behavior"] == "allow"));
    let other_node = approvals.submit(&request(&rid, "r1", "n2", bash_args("c0")));
    assert!(matches!(other_node, Submitted::Wait(..)));
    let err = approvals
        .answer(&PermissionAnswer {
            id: first.id,
            allow: true,
            scope: PermissionScope::Once,
            message: None,
        })
        .unwrap_err();
    assert_eq!(err.code, "not_found");
    // Dropping the registration denies the rest with "cancelled".
    drop(reg);
    assert!(approvals.list().is_empty());
    let d = waiting.remove(0).await.unwrap();
    assert_eq!(d, json!({"behavior":"deny","message":CANCELLED}));
    let kinds: Vec<NodeEventKind> = rec.events.lock().unwrap().iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&NodeEventKind::PermissionAnswered));
}

#[tokio::test]
async fn wrong_token_is_closed_without_reply() {
    let tmp = tempfile::tempdir().unwrap();
    let approvals = Approvals::new(tmp.path().to_path_buf());
    let rec = Arc::new(Rec::default());
    let reg = approvals
        .register("r1", tmp.path(), Path::new("/app"), rec.clone())
        .unwrap();
    let cfg: Value = serde_json::from_str(&reg.mcp_config("n1")).unwrap();
    let env = cfg["mcpServers"][SERVER_NAME]["env"].clone();
    let addr = env[ENV_ADDR].as_str().unwrap().to_string();
    let line = json!({"token":"00ff","run_id":"r1","node_id":"n1","reg":env[ENV_REG],
        "arguments":bash_args("x")});
    let reply = tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        writeln!(s, "{line}").unwrap();
        let mut out = String::new();
        BufReader::new(s).read_line(&mut out).unwrap();
        out
    })
    .await
    .unwrap();
    assert_eq!(reply, "");
    assert!(rec.prompts.lock().unwrap().is_empty());
    // The right token through the real relay: the prompt arrives; a deny answers it.
    let relay = TcpRelay::from_env(|k| env[k].as_str().map(String::from)).unwrap();
    let asked = tokio::task::spawn_blocking(move || relay.ask(&bash_args("echo hi")));
    let prompt = loop {
        if let Some(p) = approvals.list().pop() {
            break p;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(
        (prompt.summary.as_str(), prompt.node_id.as_str()),
        ("echo hi", "n1")
    );
    approvals
        .answer(&PermissionAnswer {
            id: prompt.id,
            allow: false,
            scope: PermissionScope::Once,
            message: Some("not now".into()),
        })
        .unwrap();
    assert_eq!(
        asked.await.unwrap(),
        json!({"behavior":"deny","message":"not now"})
    );
}

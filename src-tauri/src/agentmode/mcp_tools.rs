//! The hidden `claude-graph --mcp-graph` mode: a stdio MCP server whose
//! tools drive an agent run (names are a public API; docs/cli-addon.md).
//! Calls are short file operations, answered in order on the reading thread.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::contract::{AppError, AppResult};
use crate::mcp::{error, reply, text_result, write_line, METHOD_NOT_FOUND};

use super::report::{Outcome, ReportInput};
use super::{
    client_name, next, open, report, run, text, Caller, Channel, FLAG_MCP_GRAPH, NEXT_MAX,
};

/// The loop, sent as the server's `instructions` and as the prompt `graph_run`.
pub const INSTRUCTIONS: &str = "\
Claude Graph schedules a graph of tasks; you do the work in this session, one node at a time.
1. graph_open {dir}: dir = absolute path of the graph folder. Keep the run_id it returns.
2. graph_next {dir, run_id}: returns the brief of the next ready node(s) (marked running).
3. Do the work the brief describes yourself; write its deliverables into its deliverables folder.
4. graph_report {dir, run_id, node_id, status: \"done\", summary}: summary = what you did and where.
5. Repeat graph_next / work / graph_report until graph_next answers done: true.
Only work on nodes graph_next handed you; it enforces dependencies and limits.
If a brief is unclear, ask the user in this chat. If you still cannot proceed, report status
\"blocked\" with your question; once the user answers, call graph_next with retry: <node_id>.
Report \"failed\" when you cannot do it (dependents are skipped; retry works the same way).
graph_note adds a line the user sees in the Claude Graph app; graph_status shows the table.";

pub const PROMPT_NAME: &str = "graph_run";
pub const TOOL_NAMES: [&str; 5] = [
    "graph_open",
    "graph_next",
    "graph_report",
    "graph_note",
    "graph_status",
];

fn dir_prop() -> Value {
    json!({ "type": "string", "description": "Absolute path of the graph folder (it holds graph.json)." })
}

fn run_prop() -> Value {
    json!({ "type": "string", "description": "run_id returned by graph_open." })
}

/// The `tools/list` entries.
pub fn tools() -> Value {
    json!([
        {
            "name": "graph_open",
            "description": "Open a Claude Graph folder: returns the graph's nodes (goal, dependencies, resource tags, deliverables) and the run_id of the agent run (an unfinished one is resumed). Call this first.",
            "inputSchema": { "type": "object", "properties": {
                "dir": dir_prop(),
                "new_run": { "type": "boolean", "description": "End the unfinished agent run and start a new one." }
            }, "required": ["dir"], "additionalProperties": false }
        },
        {
            "name": "graph_next",
            "description": "Get the next ready node(s) of the run: the full brief of each (marked running). Answers with what the run waits on when nothing is ready, and done: true when every node is settled.",
            "inputSchema": { "type": "object", "properties": {
                "dir": dir_prop(),
                "run_id": run_prop(),
                "max": { "type": "integer", "minimum": 1, "maximum": NEXT_MAX, "default": 1, "description": "How many ready nodes to take at once (for parallel work)." },
                "retry": { "type": "string", "description": "Node id reported failed or blocked to hand out again." }
            }, "required": ["dir", "run_id"], "additionalProperties": false }
        },
        {
            "name": "graph_report",
            "description": "Report how a handed-out node ended. done: the app checks the required deliverables (missing ones make it failed). failed: dependents are skipped. blocked: dependents wait; give the question for the user.",
            "inputSchema": { "type": "object", "properties": {
                "dir": dir_prop(),
                "run_id": run_prop(),
                "node_id": { "type": "string" },
                "status": { "type": "string", "enum": ["done", "failed", "blocked"] },
                "summary": { "type": "string", "description": "What you did and where the results are; downstream nodes read it." },
                "question": { "type": "string", "description": "With status blocked: the question for the user." },
                "deliverables_written": { "type": "array", "items": { "type": "string" }, "description": "Files you wrote (relative to the node's deliverables folder, or absolute)." }
            }, "required": ["dir", "run_id", "node_id", "status", "summary"], "additionalProperties": false }
        },
        {
            "name": "graph_note",
            "description": "Add a progress line to the run log shown in the Claude Graph app.",
            "inputSchema": { "type": "object", "properties": {
                "dir": dir_prop(),
                "run_id": run_prop(),
                "node_id": { "type": "string", "description": "The node the note is about (optional)." },
                "text": { "type": "string" }
            }, "required": ["dir", "run_id", "text"], "additionalProperties": false }
        },
        {
            "name": "graph_status",
            "description": "Compact table of node statuses of the run (default: the newest agent run of the folder).",
            "inputSchema": { "type": "object", "properties": {
                "dir": dir_prop(),
                "run_id": run_prop()
            }, "required": ["dir"], "additionalProperties": false }
        }
    ])
}

fn prompt_text(dir: Option<&str>) -> String {
    match dir.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => {
            format!("Run the Claude Graph in {d} with the claude-graph tools.\n\n{INSTRUCTIONS}")
        }
        None => format!("Run a Claude Graph with the claude-graph tools.\n\n{INSTRUCTIONS}"),
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn required<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    arg_str(args, key)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::new("invalid_input", format!("missing string argument {key:?}")))
}

fn graph_dir(args: &Value) -> AppResult<PathBuf> {
    run::graph_dir(required(args, "dir")?, false)
}

fn report_input(args: &Value) -> AppResult<ReportInput> {
    let word = required(args, "status")?;
    let outcome = Outcome::parse(word)
        .ok_or_else(|| AppError::new("invalid_input", "status must be done, failed or blocked"))?;
    let written = match args.get("deliverables_written") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                AppError::new("invalid_input", "deliverables_written must be strings")
            })?,
        Some(_) => {
            return Err(AppError::new(
                "invalid_input",
                "deliverables_written must be a list",
            ))
        }
    };
    Ok(ReportInput {
        node_id: required(args, "node_id")?.trim().to_string(),
        outcome,
        summary: arg_str(args, "summary").unwrap_or_default().to_string(),
        question: arg_str(args, "question").map(str::to_string),
        deliverables_written: written,
    })
}

/// Run one tool; the text for the model.
pub fn call(name: &str, args: &Value, caller: &Caller) -> AppResult<String> {
    match name {
        "graph_open" => {
            let new_run = args
                .get("new_run")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let o = open::open(&graph_dir(args)?, caller, new_run)?;
            Ok(format!(
                "{}\n{}",
                text::overview(&o),
                json!({ "run_id": o.run_id })
            ))
        }
        "graph_next" => {
            let max = args.get("max").and_then(Value::as_u64).unwrap_or(1);
            let max = usize::try_from(max).unwrap_or(NEXT_MAX);
            let n = next::next(
                &graph_dir(args)?,
                required(args, "run_id")?,
                caller,
                max,
                arg_str(args, "retry"),
            )?;
            Ok(text::next(&n))
        }
        "graph_report" => {
            let input = report_input(args)?;
            let r = report::report(&graph_dir(args)?, required(args, "run_id")?, caller, &input)?;
            Ok(text::report(&r))
        }
        "graph_note" => {
            open::note(
                &graph_dir(args)?,
                Some(required(args, "run_id")?),
                arg_str(args, "node_id"),
                required(args, "text")?,
            )?;
            Ok("Noted.".into())
        }
        "graph_status" => Ok(text::status(&open::status(
            &graph_dir(args)?,
            arg_str(args, "run_id"),
        )?)),
        other => Err(AppError::new(
            "not_found",
            format!("unknown tool {other:?}"),
        )),
    }
}

/// Server state: who the client is (from `initialize`).
pub struct Server {
    caller: Caller,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            caller: Caller {
                client: client_name(""),
                channel: Channel::Mcp,
            },
        }
    }
}

impl Server {
    /// The response to one input line (`None`: nothing to write).
    pub fn handle(&mut self, line: &str) -> Option<Value> {
        let req = crate::mcp::parse(line)?;
        let (id, params) = (&req.id, req.params.as_ref());
        let param = |k: &str| params.and_then(|p| p.get(k));
        Some(match req.method.as_str() {
            "initialize" => {
                if let Some(name) = param("clientInfo")
                    .and_then(|c| c.get("name"))
                    .and_then(Value::as_str)
                {
                    self.caller.client = client_name(name);
                }
                crate::mcp::initialize(
                    id,
                    params,
                    json!({ "tools": {}, "prompts": {} }),
                    Some(INSTRUCTIONS),
                )
            }
            "ping" => reply(id, json!({})),
            "tools/list" => reply(id, json!({ "tools": tools() })),
            "prompts/list" => reply(
                id,
                json!({ "prompts": [{
                    "name": PROMPT_NAME,
                    "description": "Run a Claude Graph folder node by node with the claude-graph tools.",
                    "arguments": [{ "name": "dir", "description": "Absolute path of the graph folder.", "required": false }]
                }]}),
            ),
            "prompts/get" => {
                if param("name").and_then(Value::as_str) != Some(PROMPT_NAME) {
                    return Some(error(id, crate::mcp::INVALID_PARAMS, "unknown prompt"));
                }
                let dir = param("arguments")
                    .and_then(|a| a.get("dir"))
                    .and_then(Value::as_str);
                reply(
                    id,
                    json!({
                        "description": "Run a Claude Graph",
                        "messages": [{ "role": "user", "content": { "type": "text", "text": prompt_text(dir) } }]
                    }),
                )
            }
            "tools/call" => {
                let name = param("name").and_then(Value::as_str).unwrap_or_default();
                let empty = json!({});
                let args = param("arguments").unwrap_or(&empty);
                match call(name, args, &self.caller) {
                    Ok(text) => text_result(id, &text, false),
                    Err(e) => text_result(id, &format!("Error ({}): {}", e.code, e.message), true),
                }
            }
            _ => error(id, METHOD_NOT_FOUND, "method not found"),
        })
    }
}

/// Serve until `input` ends.
pub fn serve<R: BufRead, W: Write>(input: R, output: W) {
    let out = Mutex::new(output);
    let mut server = Server::default();
    for line in input.lines() {
        let Ok(line) = line else { break };
        if let Some(msg) = server.handle(&line) {
            write_line(&out, &msg, FLAG_MCP_GRAPH);
        }
    }
}

/// `Some(exit code)` when argv (after the executable) is exactly `--mcp-graph`.
pub fn from_process() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 1 || args[0] != FLAG_MCP_GRAPH {
        return None;
    }
    let stdin = std::io::stdin();
    serve(stdin.lock(), std::io::stdout());
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_name_every_tool_with_object_inputs() {
        let tools = tools();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOL_NAMES);
        for t in tools.as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object");
            let req = t["inputSchema"]["required"].as_array().unwrap();
            assert!(req.contains(&json!("dir")));
            assert!(!t["description"].as_str().unwrap().is_empty());
        }
        assert!(INSTRUCTIONS.lines().count() <= 12);
    }

    #[test]
    fn initialize_names_the_client_and_carries_instructions() {
        let mut s = Server::default();
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"claude-code","version":"2"}}}"#;
        let r = s.handle(init).unwrap();
        assert_eq!(r["result"]["instructions"], INSTRUCTIONS);
        assert!(r["result"]["capabilities"]["prompts"].is_object());
        assert_eq!(s.caller.client, "Claude Code");
        assert!(s
            .handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
        let p = s
            .handle(r#"{"jsonrpc":"2.0","id":2,"method":"prompts/get","params":{"name":"graph_run","arguments":{"dir":"C:/g"}}}"#)
            .unwrap();
        let text = p["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("C:/g") && text.contains("graph_next"));
        let bad = s
            .handle(r#"{"jsonrpc":"2.0","id":"x","method":"server/discover"}"#)
            .unwrap();
        assert_eq!(bad["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn tool_errors_are_results_not_protocol_errors() {
        let mut s = Server::default();
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"graph_open","arguments":{"dir":"relative/x"}}}"#)
            .unwrap();
        assert_eq!(r["result"]["isError"], true);
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"graph_report","arguments":{"dir":"C:/x","run_id":"r","node_id":"n","status":"maybe","summary":""}}}"#)
            .unwrap();
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("status must be"));
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#)
            .unwrap();
        assert_eq!(r["result"]["isError"], true);
    }
}

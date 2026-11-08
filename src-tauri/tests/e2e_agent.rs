//! Agent-called mode end to end with the built binary: the `--mcp-graph`
//! server over stdio (initialize, tools/list, the whole loop on a 3-node
//! graph with a resource tag) and the `graph …` CLI (exit codes, --json).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

const APP_EXE: &str = env!("CARGO_BIN_EXE_claude-graph");

/// a -> b; c on its own; b and c share the one "lab" slot.
fn fixture(dir: &Path) -> PathBuf {
    let graph = json!({
        "version": 1, "name": "agent-fixture",
        "settings": { "max_parallel": 3, "resources": { "lab": 1 } },
        "nodes": [
            { "id": "a", "title": "A", "prompt": "Write a.txt", "deliverables": [{ "path": "a.txt" }] },
            { "id": "b", "title": "B", "prompt": "Write b.txt", "resources": ["lab"] },
            { "id": "c", "title": "C", "prompt": "Write c.txt", "resources": ["lab"] }
        ],
        "edges": [{ "id": "e1", "from": "a", "to": "b" }]
    });
    std::fs::write(dir.join("graph.json"), graph.to_string()).unwrap();
    std::fs::canonicalize(dir)
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
        })
        .unwrap()
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(APP_EXE)
            .arg("--mcp-graph")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], id);
        v
    }

    /// A tools/call; (text, is_error).
    fn call(&mut self, name: &str, args: Value) -> (String, bool) {
        let v = self.request("tools/call", json!({ "name": name, "arguments": args }));
        let text = v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        (text, v["result"]["isError"].as_bool().unwrap_or(false))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn run_id_of(text: &str) -> String {
    let line = text
        .lines()
        .rev()
        .find(|l| l.contains("\"run_id\""))
        .unwrap();
    let v: Value = serde_json::from_str(line).unwrap();
    v["run_id"].as_str().unwrap().to_string()
}

fn handed(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("===== node "))
        .map(|l| l.split_whitespace().next().unwrap().to_string())
        .collect()
}

#[test]
fn mcp_server_runs_the_whole_loop() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path());
    let d = dir.to_string_lossy().to_string();
    let mut s = Server::start();
    let init = s.request(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "claude-code", "version": "2.1" } }),
    );
    assert!(init["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("graph_next"));
    writeln!(
        s.stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    let tools = s.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "graph_open",
            "graph_next",
            "graph_report",
            "graph_note",
            "graph_status"
        ]
    );
    let prompts = s.request("prompts/list", json!({}));
    assert_eq!(prompts["result"]["prompts"][0]["name"], "graph_run");

    let (text, err) = s.call("graph_open", json!({ "dir": d }));
    assert!(!err, "{text}");
    let rid = run_id_of(&text);

    // a and c are ready; c holds the lab slot.
    let (text, _) = s.call("graph_next", json!({ "dir": d, "run_id": rid, "max": 3 }));
    assert_eq!(handed(&text), ["a", "c"]);
    assert!(text.contains("graph_report"));
    std::fs::write(dir.join("nodes/a/deliverables/a.txt"), "A").unwrap();
    let (text, _) = s.call(
        "graph_report",
        json!({ "dir": d, "run_id": rid, "node_id": "a", "status": "done",
                "summary": "wrote a.txt", "deliverables_written": ["a.txt"] }),
    );
    assert!(text.contains("Node a is now succeeded"), "{text}");
    // b is ready but the lab slot is held by c.
    let (text, _) = s.call("graph_next", json!({ "dir": d, "run_id": rid }));
    assert!(
        handed(&text).is_empty() && text.contains("Running (report these first): c"),
        "{text}"
    );
    let (_, err) = s.call(
        "graph_note",
        json!({ "dir": d, "run_id": rid, "node_id": "c", "text": "working on c" }),
    );
    assert!(!err);
    s.call(
        "graph_report",
        json!({ "dir": d, "run_id": rid, "node_id": "c", "status": "done", "summary": "c ok" }),
    );
    let (text, _) = s.call("graph_next", json!({ "dir": d, "run_id": rid }));
    assert_eq!(handed(&text), ["b"]);
    assert!(text.contains("wrote a.txt"), "upstream reply inlined");
    let (text, _) = s.call(
        "graph_report",
        json!({ "dir": d, "run_id": rid, "node_id": "b", "status": "done", "summary": "b ok" }),
    );
    assert!(text.contains("the run is finished"), "{text}");
    let (text, _) = s.call("graph_next", json!({ "dir": d, "run_id": rid }));
    assert!(text.contains("All nodes are settled"), "{text}");
    let (text, _) = s.call("graph_status", json!({ "dir": d }));
    assert!(
        text.contains("succeeded") && text.contains("Claude Code"),
        "{text}"
    );

    // Errors are tool results, not protocol errors.
    let (text, err) = s.call("graph_open", json!({ "dir": "relative/dir" }));
    assert!(err && text.contains("absolute"), "{text}");
}

fn cli(dir: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(APP_EXE)
        .arg("graph")
        .args(
            args.iter()
                .map(|a| a.replace("<dir>", &dir.to_string_lossy())),
        )
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

#[test]
fn cli_exit_codes_and_json() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path());
    assert_eq!(cli(&dir, &["fly", "<dir>"]).0, 2);
    assert_eq!(cli(&dir, &["next", "<dir>"]).0, 1, "no run yet");
    let (code, out) = cli(&dir, &["open", "<dir>", "--json", "--client", "dsh"]);
    assert_eq!(code, 0);
    let o: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(o["nodes"].as_array().unwrap().len(), 3);
    let (code, out) = cli(&dir, &["next", "<dir>", "--json", "--max", "3"]);
    assert_eq!(code, 0);
    let n: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(n["nodes"].as_array().unwrap().len(), 2);
    assert!(n["nodes"][0]["brief"]
        .as_str()
        .unwrap()
        .contains("graph report"));
    assert_eq!(cli(&dir, &["next", "<dir>"]).0, 3, "nothing ready");
    // Missing deliverable: failed; b is skipped.
    let (code, out) = cli(
        &dir,
        &[
            "report",
            "<dir>",
            "a",
            "--status",
            "done",
            "--summary",
            "x",
            "--json",
        ],
    );
    assert_eq!(code, 0);
    let r: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(r["status"], "failed");
    let summary = tmp.path().join("summary.txt");
    std::fs::write(&summary, "c is fine").unwrap();
    let s = summary.to_string_lossy().to_string();
    let (code, _) = cli(
        &dir,
        &[
            "report",
            "<dir>",
            "c",
            "--status",
            "done",
            "--summary-file",
            &s,
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(cli(&dir, &["next", "<dir>"]).0, 4, "all settled");
    let (code, out) = cli(&dir, &["status", "<dir>"]);
    assert_eq!(code, 0);
    assert!(out.contains("skipped") && out.contains("dsh"), "{out}");
    // Retry reopens the failed run.
    std::fs::write(dir.join("nodes/a/deliverables/a.txt"), "A").unwrap();
    let (code, out) = cli(&dir, &["next", "<dir>", "--retry", "a"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("===== node a"));
    std::fs::write(dir.join("nodes/a/deliverables/a.txt"), "A2").unwrap();
    let (code, out) = cli(
        &dir,
        &[
            "report",
            "<dir>",
            "a",
            "--status",
            "done",
            "--summary",
            "ok",
        ],
    );
    assert_eq!(code, 0);
    assert!(out.contains("Ready now: b"), "{out}");
    let (code, out) = cli(&dir, &["note", "<dir>", "almost", "there"]);
    assert_eq!((code, out.trim()), (0, "Noted."));
}

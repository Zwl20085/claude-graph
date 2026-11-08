//! Terminal-mode helpers for the end-to-end tests: graphs built from JSON
//! nodes, running a target to the end, the fake CLI's received prompts, and
//! session process checks.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_graph_lib::contract::{Graph, RunState, RunTarget};
use claude_graph_lib::run_api::AppState;
use serde_json::{json, Value};

use super::{finish, Recorder, TIMEOUT};

/// `nodes`: JSON node objects (id, prompt, position, …); `edges`: (from, to).
pub fn graph(nodes: Vec<Value>, edges: &[(&str, &str)], settings: Value) -> Graph {
    serde_json::from_value(json!({
        "version": 1,
        "name": "term",
        "settings": settings,
        "nodes": nodes,
        "edges": edges.iter().enumerate().map(|(i, (f, t))| json!({
            "id": format!("e{i}"), "from": f, "to": t
        })).collect::<Vec<_>>(),
    }))
    .unwrap()
}

pub fn node(id: &str, x: f64, y: f64, extra: Value) -> Value {
    let mut v = json!({ "id": id, "title": format!("T {id}"), "position": { "x": x, "y": y } });
    if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
        m.extend(e);
    }
    v
}

pub fn settings(max_parallel: u32, stop_on_failure: bool) -> Value {
    json!({ "max_parallel": max_parallel, "stop_on_failure": stop_on_failure,
            "default_permission_mode": "claude_settings" })
}

pub async fn run(
    app: &AppState,
    dir: &Path,
    target: RunTarget,
    rec: &Arc<Recorder>,
) -> (String, RunState) {
    let (run_id, join) = app.start_by_mode(dir, target, rec.clone()).unwrap();
    (run_id, finish(join).await)
}

/// The session's hooks folder (outside the graph folder), from the pointer
/// file the session leaves in `runs/<run>/sessions/<sid>/hooks.path`.
pub fn hooks_dir(dir: &Path, run_id: &str, sid: &str) -> std::path::PathBuf {
    let pointer = dir
        .join("runs")
        .join(run_id)
        .join("sessions")
        .join(sid)
        .join("hooks.path");
    let text = fs::read_to_string(&pointer).unwrap_or_default();
    std::path::PathBuf::from(text.trim())
}

pub fn received(dir: &Path, run_id: &str, sid: &str) -> Vec<String> {
    let path = hooks_dir(dir, run_id, sid).join("fake-received.log");
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

pub async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub fn process_alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    } else {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    }
}

/// Kill the run's sessions and wait for their processes to be gone.
pub async fn kill_sessions(app: &AppState, run_id: &str) {
    app.cancel_run(run_id).unwrap();
    wait_until("session tasks to end", || app.terminals().live_count() == 0).await;
}

//! Smoke tests against the real `claude` CLI (the owner's own login). Ignored by
//! default; run with `cargo test --test real_claude -- --ignored --nocapture`.
//! Each one costs a tiny haiku turn.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_graph_lib::claude_bin::ENV_CLAUDE_BIN;
use claude_graph_lib::contract::{Graph, NodeEventKind, NodeStatus, RunTarget};
use claude_graph_lib::run_api::AppState;
use claude_graph_lib::run_store;
use common::*;

fn one_node_graph(id: &str, prompt: &str, deliverable: Option<&str>) -> Graph {
    let deliverables = match deliverable {
        Some(p) => serde_json::json!([{ "path": p, "description": "" }]),
        None => serde_json::json!([]),
    };
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "name": "real-smoke",
        "settings": { "max_parallel": 1 },
        "nodes": [{
            "id": id,
            "title": id,
            "prompt": prompt,
            "model": "haiku",
            "permission_mode": "acceptEdits",
            "max_turns": 5,
            "deliverables": deliverables,
        }],
        "edges": [],
    }))
    .expect("graph json")
}

fn use_real_claude() {
    std::env::remove_var(ENV_CLAUDE_BIN);
}

fn print_node_files(dir: &std::path::Path, run_id: &str, node: &str) {
    let run_dir = dir.join("runs").join(run_id).join(node);
    for f in ["summary.json", "stderr.log", "result.md"] {
        let text = std::fs::read_to_string(run_dir.join(f)).unwrap_or_default();
        println!("--- {f} ---\n{text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real claude CLI (one haiku turn)"]
async fn real_claude_smoke() {
    use_real_claude();
    let graph = one_node_graph(
        "hello",
        "Create a file named hello.md in your deliverables folder containing the single line 'hello from claude graph'. Then finish.",
        Some("hello.md"),
    );
    let tmp = graph_dir(&graph);
    let dir = tmp.path();
    let recorder = Arc::new(Recorder::default());
    let (run_id, join) = AppState::default()
        .start_run(dir, RunTarget::All, recorder.clone())
        .expect("start_run");
    let state = finish(join).await;
    for line in recorder.events_of("hello", NodeEventKind::System) {
        if line.starts_with("spawn:") {
            println!("{line}");
        }
    }
    print_node_files(dir, &run_id, "hello");
    for e in recorder.events.lock().unwrap().iter() {
        let t: String = e.text.chars().take(300).collect();
        println!("[{:?}] {t}", e.kind);
    }
    let node = &state.nodes["hello"];
    assert_eq!(node.status, NodeStatus::Succeeded, "{:?}", node.reason);
    let file = dir.join("nodes/hello/deliverables/hello.md");
    let text = std::fs::read_to_string(&file).expect("hello.md written");
    println!("--- hello.md ---\n{text}");
    assert!(text.contains("hello from claude graph"));
    let summary = run_store::read_node_result(dir, &run_id, "hello")
        .unwrap()
        .summary
        .expect("summary.json");
    println!("cost_usd = {:?}", summary.cost_usd);
}

#[cfg(windows)]
fn claude_exe_count() -> usize {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq claude.exe", "/NH"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.to_ascii_lowercase().starts_with("claude.exe"))
        .count()
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real claude CLI and cancels it"]
async fn real_claude_cancel_kills_tree() {
    use_real_claude();
    let graph = one_node_graph(
        "slow",
        "Count slowly from 1 to 200, one number per line, using a separate Bash `echo` call for each number. Then finish.",
        None,
    );
    let tmp = graph_dir(&graph);
    let dir = tmp.path();
    let before = claude_exe_count();
    let app = AppState::default();
    let recorder = Arc::new(Recorder::default());
    let (run_id, join) = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .expect("start_run");
    // Wait for the CLI's own system init event (not our "spawn:" line).
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let init = recorder
            .events_of("slow", NodeEventKind::System)
            .iter()
            .any(|t| !t.starts_with("spawn:"));
        if init {
            break;
        }
        assert!(Instant::now() < deadline, "no system init event in 60 s");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let during = claude_exe_count();
    let t0 = Instant::now();
    app.cancel_run(&run_id).expect("cancel_run");
    let state = finish(join).await;
    let took = t0.elapsed();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after = claude_exe_count();
    println!(
        "claude.exe count: before={before} during={during} after={after}; cancel took {took:?}"
    );
    print_node_files(dir, &run_id, "slow");
    assert_eq!(state.nodes["slow"].status, NodeStatus::Cancelled);
    assert_eq!(during, before + 1, "the run's claude.exe was visible");
    assert_eq!(
        after, before,
        "taskkill /T removed claude.exe behind the claude.cmd shim"
    );
}

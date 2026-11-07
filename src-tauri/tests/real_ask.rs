//! "Ask me in the app" against the real `claude` CLI (the owner's login), with
//! the built app binary as the approval server. Ignored by default; run with
//! `cargo test --test real_ask -- --ignored --nocapture` (one haiku turn,
//! about one cent). The wire format it checks is the one recorded with
//! scripts/probe-prompt-tool.mjs in docs/CLAUDE_CLI_NOTES.md.
//!
//! The command is `mkdir`, not `echo`: on the owner's machine `echo` is
//! allowed by the user's own settings and never reaches the prompt tool.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_graph_lib::claude_bin::ENV_CLAUDE_BIN;
use claude_graph_lib::contract::{
    Graph, NodeEventKind, NodeStatus, PermissionAnswer, PermissionScope, RunTarget,
};
use claude_graph_lib::run_api::{abs, AppState};
use common::*;
use serde_json::json;

const APP_EXE: &str = env!("CARGO_BIN_EXE_claude-graph");
const DIR_NAME: &str = "ask-app-probe-dir";

fn graph() -> Graph {
    serde_json::from_value(json!({
        "version": 1,
        "name": "real-ask",
        "settings": { "max_parallel": 1, "run_mode": "headless" },
        "nodes": [{
            "id": "ask",
            "title": "ask",
            "prompt": format!("Run exactly this shell command with the Bash tool, once, in the current directory: mkdir {DIR_NAME}. Do nothing else. Then finish."),
            "model": "haiku",
            "permission_mode": "ask_app",
            "max_turns": 5,
        }],
        "edges": [],
    }))
    .expect("graph json")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real claude CLI (one haiku turn)"]
async fn real_ask_app_allow() {
    std::env::remove_var(ENV_CLAUDE_BIN);
    let tmp = graph_dir(&graph());
    let dir = abs(tmp.path());
    let data = tempfile::tempdir().unwrap();
    let app = AppState::new(data.path().to_path_buf()).with_notify_exe(PathBuf::from(APP_EXE));
    let rec = Arc::new(Recorder::default());
    let (run_id, join) = app
        .start_run(&dir, RunTarget::All, rec.clone())
        .expect("start_run");
    let started = Instant::now();
    let prompt = loop {
        if let Some(p) = app.approvals().list().first() {
            break p.clone();
        }
        if join.is_finished() || started.elapsed() > Duration::from_secs(120) {
            for e in rec.events.lock().unwrap().iter() {
                println!(
                    "[{:?}] {}",
                    e.kind,
                    e.text.chars().take(300).collect::<String>()
                );
            }
            panic!("no permission prompt arrived");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    println!("prompt: {}", serde_json::to_string_pretty(&prompt).unwrap());
    // The recorded format: tool_name + input {command, description}.
    assert_eq!(
        (prompt.run_id.as_str(), prompt.node_id.as_str()),
        (run_id.as_str(), "ask")
    );
    assert_eq!(prompt.tool_name, "Bash");
    let command = prompt.input["command"].as_str().unwrap_or_default();
    assert!(command.contains(&format!("mkdir {DIR_NAME}")), "{command}");
    assert!(prompt.summary.contains(DIR_NAME));
    app.approvals()
        .answer(&PermissionAnswer {
            id: prompt.id.clone(),
            allow: true,
            scope: PermissionScope::Once,
            message: None,
        })
        .unwrap();
    let state = finish(join).await;
    for e in rec.events.lock().unwrap().iter() {
        println!(
            "[{:?}] {}",
            e.kind,
            e.text.chars().take(300).collect::<String>()
        );
    }
    let node = &state.nodes["ask"];
    assert_eq!(node.status, NodeStatus::Succeeded, "{:?}", node.reason);
    assert!(dir.join(DIR_NAME).is_dir(), "the allowed command ran");
    assert_eq!(
        rec.events_of("ask", NodeEventKind::PermissionAnswered),
        ["allowed (once)"]
    );
}

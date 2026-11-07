//! Terminal mode against the real `claude` CLI (the owner's own login), in a
//! real pty. Ignored by default; run with
//! `cargo test --test real_terminal -- --ignored --nocapture` (about one cent).
//!
//! A fresh temp folder makes Claude Code ask whether to trust it (and maybe
//! whether to use a project MCP server). The test answers those dialogs the
//! way a user would in the window: never the "No, exit" choice.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_graph_lib::claude_bin::ENV_CLAUDE_BIN;
use claude_graph_lib::contract::{Graph, NodeStatus, RunTarget};
use claude_graph_lib::run_api::AppState;
use claude_graph_lib::terminal::api as term;
use claude_graph_lib::terminal::screen::plain_tail;
use common::*;

const LIMIT: Duration = Duration::from_secs(240);

fn one_node_graph() -> Graph {
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "name": "real-terminal",
        "settings": { "max_parallel": 1, "default_permission_mode": "claude_settings" },
        "nodes": [{
            "id": "ok",
            "title": "Say ok",
            "prompt": "Reply with the single word ok and create no files",
        }],
        "edges": [],
    }))
    .expect("graph json")
}

/// Answer an open dialog like a user: Enter, or Down + Enter when the
/// highlighted choice is a "No".
async fn answer_dialogs(app: &AppState, run_id: &str) {
    let Ok(session) = app.terminals().get(run_id, "s1") else {
        return;
    };
    let Some(pty) = session.pty() else { return };
    if !pty.output.dialog_open() {
        return;
    }
    let tail = plain_tail(&pty.output.replay().text, 400).to_lowercase();
    let highlighted = tail
        .rsplit('❯')
        .next()
        .unwrap_or("")
        .trim_start()
        .to_string();
    println!("dialog: …{}", plain_tail(&tail, 200));
    if highlighted.starts_with("no") {
        term::pty_write(app.terminals(), run_id, "s1", "\u{1b}[B").unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    term::pty_write(app.terminals(), run_id, "s1", "\r").unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real claude CLI interactively (one short reply)"]
async fn real_claude_in_a_pty_reports_start_and_stop() {
    std::env::remove_var(ENV_CLAUDE_BIN);
    let tmp = graph_dir(&one_node_graph());
    let dir = tmp.path();
    let app = trusted_app(dir);
    let rec = Arc::new(Recorder::default());
    let t0 = Instant::now();
    let (run_id, mut join) = app.start_by_mode(dir, RunTarget::All, rec.clone()).unwrap();
    let state = loop {
        assert!(t0.elapsed() < LIMIT, "run did not finish in {LIMIT:?}");
        tokio::select! {
            done = &mut join => break done.unwrap(),
            _ = tokio::time::sleep(Duration::from_millis(500)) => answer_dialogs(&app, &run_id).await,
        }
    };
    let session_dir = dir.join("runs").join(&run_id).join("sessions").join("s1");
    let hooks = std::fs::read_to_string(session_dir.join("hooks.log")).unwrap_or_default();
    let transcript =
        std::fs::read_to_string(session_dir.join("transcript.log")).unwrap_or_default();
    let plain = plain_tail(&transcript, 3000);
    println!("--- hooks.log ---\n{hooks}--- transcript (plain tail) ---\n{plain}");
    println!(
        "run {run_id}: {:?} in {:?}; node {:?}",
        state.status,
        t0.elapsed(),
        state.nodes["ok"]
    );
    let words: Vec<&str> = hooks.lines().map(str::trim).collect();
    assert!(words.contains(&"start"), "no start hook line");
    assert!(words.contains(&"stop"), "no stop hook line");
    let replied_ok = plain
        .split('●')
        .skip(1)
        .any(|p| p.trim_start().to_lowercase().starts_with("ok"));
    assert!(replied_ok, "transcript has no '● ok' reply");
    assert_eq!(state.nodes["ok"].status, NodeStatus::Succeeded);
    app.cancel_run(&run_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.terminals().live_count() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(app.terminals().live_count(), 0, "session not killed");
}

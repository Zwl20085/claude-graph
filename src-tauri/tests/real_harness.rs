//! Harness profiles against the real CLIs (the owner's own logins), in a
//! real pty. Ignored by default; each costs a cent or less:
//!
//! `cargo test --test real_harness -- --ignored --nocapture --test-threads 1`
//!
//! - `real_codex`: the built-in `codex` profile (Codex CLI on PATH), one
//!   node; readiness, submit (composer check), the notify `stop` line,
//!   success and reply.md. Prints the screen rows as Codex drew them.
//! - `real_deepseek`: the built-in `claude-deepseek` profile (Claude Code
//!   against DeepSeek's Anthropic-compatible API); needs `DEEPSEEK_API_KEY`
//!   in the environment (skipped with a message when unset).

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_graph_lib::contract::{Graph, NodeEventKind, NodeStatus, RunState, RunTarget};
use claude_graph_lib::run_api::AppState;
use claude_graph_lib::terminal::api as term;
use claude_graph_lib::terminal::screen::plain_tail;
use common::term::kill_sessions;
use common::*;

const LIMIT: Duration = Duration::from_secs(240);
const APP_EXE: &str = env!("CARGO_BIN_EXE_claude-graph");
const PROMPT: &str = "Reply with the single word ok and create no files";

fn one_node(harness: &str) -> Graph {
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "name": "real-harness",
        "settings": { "max_parallel": 1, "default_permission_mode": "claude_settings",
                      "harness": harness },
        "nodes": [{ "id": "ok", "title": "Say ok", "prompt": PROMPT }],
        "edges": [],
    }))
    .expect("graph json")
}

/// Answer an open dialog like a user (Enter on the highlighted choice, Down
/// first when it is a "No" / "Quit").
async fn answer_dialogs(app: &AppState, run_id: &str) {
    let Ok(session) = app.terminals().get(run_id, "s1") else {
        return;
    };
    let Some(pty) = session.pty() else { return };
    if !pty.output.screen().dialog_open() {
        return;
    }
    let tail = plain_tail(&pty.output.replay().text, 400).to_lowercase();
    println!("answering dialog: …{}", plain_tail(&tail, 300));
    let highlighted = tail
        .rsplit(['❯', '>', '›'])
        .next()
        .unwrap_or("")
        .trim_start()
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
        .trim_start()
        .to_string();
    if highlighted.starts_with("no") || highlighted.starts_with("quit") {
        term::pty_write(app.terminals(), run_id, "s1", "\u{1b}[B").unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    term::pty_write(app.terminals(), run_id, "s1", "\r").unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
}

fn print_rows(app: &AppState, run_id: &str, label: &str) {
    let Some(pty) = app.terminals().get(run_id, "s1").ok().and_then(|s| s.pty()) else {
        return;
    };
    println!("===== {label} =====");
    for (i, r) in pty.output.rows().iter().enumerate() {
        if !r.trim().is_empty() {
            println!("{i:02}|{r}");
        }
    }
}

/// Run the one-node graph on `harness`; answers dialogs; returns the run.
async fn run_one(harness: &str) -> (tempfile::TempDir, AppState, String, RunState, Arc<Recorder>) {
    let tmp = graph_dir(&one_node(harness));
    let dir = tmp.path().to_path_buf();
    let app = trusted_app(&dir).with_notify_exe(PathBuf::from(APP_EXE));
    let rec = Arc::new(Recorder::default());
    let (run_id, mut join) = app
        .start_by_mode(&dir, RunTarget::All, rec.clone())
        .expect("run starts");
    let t0 = Instant::now();
    let mut shown_ready = false;
    let state = loop {
        tokio::select! {
            done = &mut join => break done.expect("run task"),
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                assert!(t0.elapsed() < LIMIT, "run took too long");
                answer_dialogs(&app, &run_id).await;
                let submitted = rec.events_of("ok", NodeEventKind::Text)
                    .iter().any(|t| t == "typed");
                if submitted && !shown_ready {
                    shown_ready = true;
                    print_rows(&app, &run_id, "when the message was typed");
                }
            }
        }
    };
    print_rows(&app, &run_id, "after the run");
    (tmp, app, run_id, state, rec)
}

fn check_common(dir: &Path, run_id: &str, state: &RunState, rec: &Recorder) -> String {
    let texts = rec.events_of("ok", NodeEventKind::Text);
    println!("events: {texts:?}");
    let node = &state.nodes["ok"];
    assert_eq!(node.status, NodeStatus::Succeeded, "{:?}", node.reason);
    assert!(texts.iter().any(|t| t == "submitted"), "{texts:?}");
    let hooks =
        std::fs::read_to_string(common::term::hooks_dir(dir, run_id, "s1").join("hooks.log"))
            .unwrap_or_default();
    println!("hooks.log:\n{hooks}");
    assert!(hooks.lines().any(|l| l.starts_with("stop")), "{hooks}");
    let reply = std::fs::read_to_string(dir.join("runs").join(run_id).join("ok/reply.md"))
        .expect("reply.md");
    println!("reply.md:\n{reply}");
    assert!(reply.to_lowercase().contains("ok"), "{reply}");
    reply
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real Codex CLI interactively (one short reply)"]
async fn real_codex() {
    std::env::remove_var(claude_graph_lib::harness::command::ENV_CODEX_BIN);
    let (tmp, app, run_id, state, rec) = run_one("codex").await;
    check_common(tmp.path(), &run_id, &state, &rec);
    assert_eq!(state.sessions[0].harness, "codex");
    kill_sessions(&app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real claude CLI against DeepSeek (one short reply)"]
async fn real_deepseek() {
    if std::env::var("DEEPSEEK_API_KEY").map_or(true, |v| v.trim().is_empty()) {
        println!("SKIPPED: DEEPSEEK_API_KEY is not set in this environment");
        return;
    }
    std::env::remove_var(claude_graph_lib::claude_bin::ENV_CLAUDE_BIN);
    let (tmp, app, run_id, state, rec) = run_one("claude-deepseek").await;
    check_common(tmp.path(), &run_id, &state, &rec);
    let transcript = std::fs::read_to_string(
        tmp.path()
            .join("runs")
            .join(&run_id)
            .join("sessions/s1/transcript.log"),
    )
    .unwrap()
    .to_lowercase();
    // The model line of Claude Code's banner (not, say, an MCP server name).
    let plain = plain_tail(&transcript, transcript.len());
    let at = plain
        .find("deepseek-flash")
        .expect("the model line names deepseek-flash");
    let line = plain.get(at.saturating_sub(40)..(at + 40).min(plain.len()));
    println!("model line: …{}…", line.unwrap_or("deepseek-flash"));
    kill_sessions(&app, &run_id).await;
}

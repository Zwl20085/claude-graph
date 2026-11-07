//! End-to-end: "Ask me in the app" through scripts/fake-claude.mjs (FAKE_ASK,
//! scripts/fake-ask.mjs) with the built app binary as the `--mcp-approve`
//! approval server. Needs `node`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_graph_lib::approve::args::REDACTED;
use claude_graph_lib::approve::ENV_TOKEN;
use claude_graph_lib::contract::{
    Graph, NodeEventKind, NodeStatus, PermissionAnswer, PermissionPrompt, PermissionScope,
    RunTarget,
};
use claude_graph_lib::run_api::AppState;
use common::*;
use serde_json::json;

const APP_EXE: &str = env!("CARGO_BIN_EXE_claude-graph");

/// One headless node under `ask_app` whose prompt is `prompt`.
fn ask_graph(prompt: &str) -> Graph {
    serde_json::from_value(json!({
        "version": 1,
        "name": "ask",
        "settings": { "run_mode": "headless" },
        "nodes": [{ "id": "a", "title": "A", "prompt": prompt, "permission_mode": "ask_app" }],
        "edges": []
    }))
    .unwrap()
}

fn app_with(data: &Path, dir: &Path) -> AppState {
    let app = AppState::new(data.to_path_buf()).with_notify_exe(PathBuf::from(APP_EXE));
    app.trust_graph(&claude_graph_lib::run_api::abs(dir))
        .unwrap();
    app
}

async fn next_prompt(app: &AppState) -> PermissionPrompt {
    let started = Instant::now();
    loop {
        if let Some(p) = app.approvals().list().first() {
            return p.clone();
        }
        assert!(started.elapsed() < TIMEOUT, "no permission prompt arrived");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn answer(app: &AppState, p: &PermissionPrompt, allow: bool, scope: PermissionScope, msg: &str) {
    app.approvals()
        .answer(&PermissionAnswer {
            id: p.id.clone(),
            allow,
            scope,
            message: (!msg.is_empty()).then(|| msg.to_string()),
        })
        .unwrap();
}

fn lines(path: PathBuf) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_once_runs_the_call_and_logs_both_events() {
    use_fake_claude("");
    let tmp = graph_dir(&ask_graph("Do it. FAKE_ASK=Bash"));
    let data = tempfile::tempdir().unwrap();
    let app = app_with(data.path(), tmp.path());
    let rec = Arc::new(Recorder::default());
    let (run_id, join) = app
        .start_run(tmp.path(), RunTarget::All, rec.clone())
        .unwrap();
    let p = next_prompt(&app).await;
    assert_eq!(
        (p.run_id.as_str(), p.node_id.as_str()),
        (run_id.as_str(), "a")
    );
    assert_eq!(
        (p.tool_name.as_str(), p.summary.as_str()),
        ("Bash", "echo hi")
    );
    assert_eq!(p.input, json!({"command": "echo hi"}));
    assert_eq!(rec.prompts.lock().unwrap().len(), 1);
    answer(&app, &p, true, PermissionScope::Once, "");
    let state = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    assert_eq!(
        lines(tmp.path().join("asked-allowed.txt")),
        [r#"{"command":"echo hi"}"#]
    );
    assert_eq!(
        rec.events_of("a", NodeEventKind::PermissionPrompt),
        ["Bash: echo hi"]
    );
    assert_eq!(
        rec.events_of("a", NodeEventKind::PermissionAnswered),
        ["allowed (once)"]
    );
    assert_eq!(*rec.answered.lock().unwrap(), [p.id]);
    let spawn = rec.events_of("a", NodeEventKind::System);
    let spawn = spawn.iter().find(|t| t.starts_with("spawn: ")).unwrap();
    assert!(spawn.contains("--permission-mode default"), "{spawn}");
    assert!(spawn.contains("--permission-prompt-tool mcp__claude_graph__approve"));
    assert!(
        spawn.contains(REDACTED) && !spawn.contains(ENV_TOKEN),
        "{spawn}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn deny_passes_the_message_to_the_cli() {
    use_fake_claude("");
    let tmp = graph_dir(&ask_graph("FAKE_ASK=Bash"));
    let data = tempfile::tempdir().unwrap();
    let app = app_with(data.path(), tmp.path());
    let rec = Arc::new(Recorder::default());
    let (_, join) = app
        .start_run(tmp.path(), RunTarget::All, rec.clone())
        .unwrap();
    let p = next_prompt(&app).await;
    answer(&app, &p, false, PermissionScope::Once, "use dir instead");
    finish(join).await;
    assert_eq!(
        lines(tmp.path().join("asked-denied.txt")),
        ["use dir instead"]
    );
    assert!(!tmp.path().join("asked-allowed.txt").exists());
    assert_eq!(
        rec.events_of("a", NodeEventKind::PermissionAnswered),
        ["denied (once): use dir instead"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn node_scope_answers_the_second_identical_ask() {
    use_fake_claude("");
    let tmp = graph_dir(&ask_graph("FAKE_ASK=Bash FAKE_ASK_COUNT=2"));
    let data = tempfile::tempdir().unwrap();
    let app = app_with(data.path(), tmp.path());
    let rec = Arc::new(Recorder::default());
    let (_, join) = app
        .start_run(tmp.path(), RunTarget::All, rec.clone())
        .unwrap();
    let p = next_prompt(&app).await;
    answer(&app, &p, true, PermissionScope::Node, "");
    finish(join).await;
    assert_eq!(lines(tmp.path().join("asked-allowed.txt")).len(), 2);
    assert_eq!(rec.prompts.lock().unwrap().len(), 1);
    assert_eq!(
        rec.events_of("a", NodeEventKind::PermissionAnswered),
        ["allowed (node)", "allowed (node)"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn graph_scope_survives_a_new_run() {
    use_fake_claude("");
    let tmp = graph_dir(&ask_graph("FAKE_ASK=Bash"));
    let data = tempfile::tempdir().unwrap();
    let app = app_with(data.path(), tmp.path());
    let rec = Arc::new(Recorder::default());
    let (_, join) = app
        .start_run(tmp.path(), RunTarget::All, rec.clone())
        .unwrap();
    let p = next_prompt(&app).await;
    answer(&app, &p, true, PermissionScope::Graph, "");
    finish(join).await;
    assert!(data.path().join("approvals.json").is_file());
    // A new app (new token, new listener) with the same data folder.
    let app2 = app_with(data.path(), tmp.path());
    let rec2 = Arc::new(Recorder::default());
    let (_, join) = app2
        .start_run(tmp.path(), RunTarget::All, rec2.clone())
        .unwrap();
    let state = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    assert!(rec2.prompts.lock().unwrap().is_empty());
    assert_eq!(lines(tmp.path().join("asked-allowed.txt")).len(), 2);
    assert_eq!(
        rec2.events_of("a", NodeEventKind::PermissionAnswered),
        ["allowed (graph)"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_denies_the_pending_prompt() {
    use_fake_claude("");
    let tmp = graph_dir(&ask_graph("FAKE_ASK=Bash"));
    let data = tempfile::tempdir().unwrap();
    let app = app_with(data.path(), tmp.path());
    let rec = Arc::new(Recorder::default());
    let (run_id, join) = app
        .start_run(tmp.path(), RunTarget::All, rec.clone())
        .unwrap();
    let p = next_prompt(&app).await;
    app.cancel_run(&run_id).unwrap();
    let state = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Cancelled);
    assert!(app.approvals().list().is_empty());
    assert_eq!(*rec.answered.lock().unwrap(), std::slice::from_ref(&p.id));
    assert_eq!(
        rec.events_of("a", NodeEventKind::PermissionAnswered),
        ["denied (once): cancelled"]
    );
    let err = app
        .approvals()
        .answer(&PermissionAnswer {
            id: p.id,
            allow: true,
            scope: PermissionScope::Once,
            message: None,
        })
        .unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_did_not_connect_fails_the_node() {
    use_fake_claude("");
    let tmp = graph_dir(&ask_graph("FAKE_ASK=Bash FAKE_MCP_FAIL"));
    let data = tempfile::tempdir().unwrap();
    let app = app_with(data.path(), tmp.path());
    let rec = Arc::new(Recorder::default());
    let (_, join) = app
        .start_run(tmp.path(), RunTarget::All, rec.clone())
        .unwrap();
    let state = finish(join).await;
    let node = &state.nodes["a"];
    assert_eq!(node.status, NodeStatus::Failed);
    let reason = node.reason.clone().unwrap_or_default();
    assert!(
        reason.starts_with("approval server did not connect: FAKE_MCP_FAIL"),
        "{reason}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_assistant_asks_when_the_graph_default_is_ask_app() {
    use_fake_claude("");
    let mut graph = ask_graph("unused");
    graph.settings.default_permission_mode = claude_graph_lib::contract::PermissionMode::AskApp;
    let tmp = graph_dir(&graph);
    let data = tempfile::tempdir().unwrap();
    let app = app_with(data.path(), tmp.path());
    let rec = Arc::new(Recorder::default());
    let dir = claude_graph_lib::run_api::abs(tmp.path());
    app.assistant_send(&dir, "plan it FAKE_ASK=Bash", rec.clone())
        .unwrap();
    let p = next_prompt(&app).await;
    assert_eq!(
        (p.run_id.as_str(), p.node_id.as_str()),
        ("assistant", "scheduler")
    );
    answer(&app, &p, true, PermissionScope::Once, "");
    tokio::time::timeout(TIMEOUT, app.assistant_wait(&dir))
        .await
        .unwrap();
    assert_eq!(lines(dir.join("asked-allowed.txt")).len(), 1);
}

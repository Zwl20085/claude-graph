//! Chained-headless sessions (harness kind `deepseek-harness`) with
//! `scripts/fake-dsh.mjs`: one process per message, chained by the session
//! id; questions pause like in a pty session; failures, denied approvals and
//! follow-ups from the window.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use claude_graph_lib::contract::{
    NodeEventKind, NodeStatus, PromptAnswer, RunState, RunStatus, RunTarget, TurnPrompt,
};
use claude_graph_lib::harness::command::ENV_DSH_BIN;
use claude_graph_lib::run_api::AppState;
use claude_graph_lib::terminal::api as term;
use claude_graph_lib::terminal::prompt_api;
use common::term::*;
use common::*;
use serde_json::{json, Value};

fn use_fake_dsh() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("fake-dsh.mjs")
        .canonicalize()
        .unwrap();
    let script = script.display().to_string();
    let script = script.trim_start_matches(r"\\?\");
    std::env::set_var(ENV_DSH_BIN, format!("node \"{script}\""));
}

struct Trial {
    tmp: tempfile::TempDir,
    app: AppState,
    rec: Arc<Recorder>,
}

fn trial(nodes: Vec<Value>, edges: &[(&str, &str)]) -> Trial {
    use_fake_dsh();
    let mut s = settings(1, false);
    s["harness"] = json!("deepseek-harness");
    let tmp = graph_dir(&graph(nodes, edges, s));
    let app = trusted_app(tmp.path());
    Trial {
        tmp,
        app,
        rec: Arc::new(Recorder::default()),
    }
}

fn received(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join("fake-dsh-received.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn reply(dir: &Path, run_id: &str, node: &str) -> String {
    fs::read_to_string(dir.join("runs").join(run_id).join(node).join("reply.md"))
        .unwrap_or_default()
}

fn shown(rec: &Recorder) -> String {
    rec.pty
        .lock()
        .unwrap()
        .iter()
        .map(|(_, d)| d.data.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn nodes_chain_through_one_dsh_session() {
    let nodes = vec![
        node(
            "a",
            0.0,
            0.0,
            json!({ "prompt": "remember this REMEMBER=PELICAN" }),
        ),
        node("b", 0.0, 100.0, json!({ "prompt": "what was it? RECALL" })),
    ];
    let t = trial(nodes, &[("a", "b")]);
    let dir = t.tmp.path();
    let (run_id, state) = run(&t.app, dir, RunTarget::All, &t.rec).await;
    assert_eq!(state.status, RunStatus::Succeeded, "{state:#?}");
    assert_eq!(
        received(dir),
        ["remember this REMEMBER=PELICAN", "what was it? RECALL"]
    );
    assert!(
        reply(dir, &run_id, "b").contains("PELICAN"),
        "the session id chained the turns"
    );
    assert!(reply(dir, &run_id, "a").starts_with("> remember this REMEMBER=PELICAN\n\n"));
    let window = shown(&t.rec);
    assert!(window.contains("dsh (headless, chained)"), "{window}");
    assert!(window.contains("\u{25b6} node T a") && window.contains("\u{25b6} node T b"));
    assert!(t
        .rec
        .windows
        .lock()
        .unwrap()
        .iter()
        .any(|w| w.label.ends_with("-s1")));
    let events = fs::read_to_string(dir.join("runs").join(&run_id).join("a/events.jsonl")).unwrap();
    assert!(events.lines().any(|l| l.contains("\"type\":\"session\"")));
    let session = &state.sessions[0];
    assert_eq!(session.harness, "deepseek-harness");
    // A replay of the window works without a pty.
    let replay = term::pty_replay(t.app.terminals(), &run_id, "s1").unwrap();
    assert!(replay.text.contains("PELICAN"));
    // A line typed in the window is a follow-up message in the same session.
    term::pty_write(t.app.terminals(), &run_id, "s1", "RECALL again\r").unwrap();
    wait_until("the follow-up", || received(dir).len() == 3).await;
    assert_eq!(received(dir)[2], "RECALL again");
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_question_pauses_until_answered() {
    let nodes = vec![
        node("a", 0.0, 0.0, json!({ "prompt": "set up FAKE_QUESTION" })),
        node("b", 0.0, 100.0, json!({ "prompt": "second" })),
    ];
    let t = trial(nodes, &[("a", "b")]);
    let dir = t.tmp.path();
    let (run_id, join) = t
        .app
        .start_by_mode(dir, RunTarget::All, t.rec.clone())
        .unwrap();
    wait_until("a question", || {
        !t.rec.session_prompts.lock().unwrap().is_empty()
    })
    .await;
    let p = t.rec.session_prompts.lock().unwrap()[0].clone();
    let TurnPrompt::Reply(q) = &p.prompt else {
        panic!("reply question expected")
    };
    assert_eq!(q.question, "Which framework should I use?");
    assert_eq!(q.choices.len(), 2);
    assert_eq!(received(dir).len(), 1, "paused: b is not sent");
    prompt_api::answer_prompt(
        t.app.terminals(),
        &run_id,
        "s1",
        &q.id,
        PromptAnswer::Choice(1),
    )
    .unwrap();
    let state: RunState = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    assert_eq!(state.nodes["b"].status, NodeStatus::Succeeded);
    assert_eq!(received(dir), ["set up FAKE_QUESTION", "jest", "second"]);
    assert!(reply(dir, &run_id, "a").contains("> jest\n\nThanks, using jest."));
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_and_denied_approvals_are_reported() {
    let nodes = vec![
        node("a", 0.0, 0.0, json!({ "prompt": "try FAKE_APPROVAL" })),
        node("b", 0.0, 100.0, json!({ "prompt": "break FAKE_FAIL" })),
    ];
    let t = trial(nodes, &[("a", "b")]);
    let dir = t.tmp.path();
    let (run_id, state) = run(&t.app, dir, RunTarget::All, &t.rec).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    let denied = t.rec.events_of("a", NodeEventKind::PermissionDenied);
    assert!(denied[0].starts_with("write: needs approval"), "{denied:?}");
    assert_eq!(state.nodes["b"].status, NodeStatus::Failed);
    let reason = state.nodes["b"].reason.clone().unwrap_or_default();
    assert!(
        reason.starts_with("dsh exited with code 1") && reason.contains("FAKE_FAIL"),
        "{reason}"
    );
    assert!(shown(&t.rec).contains("needs approval; headless dsh denies it"));
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn check_harness_runs_dsh_version() {
    use_fake_dsh();
    let profile = claude_graph_lib::harness::profile::builtins()
        .into_iter()
        .find(|p| p.id == "deepseek-harness")
        .unwrap();
    let check = claude_graph_lib::harness::check(&profile, None).await;
    assert!(check.ok, "{check:?}");
    assert_eq!(check.version.as_deref(), Some("0.2.1-rc.2 (fake-dsh)"));
}

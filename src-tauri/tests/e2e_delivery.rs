//! End-to-end: terminal-mode message delivery recovers from the failures
//! seen with the real CLI, simulated by scripts/fake-claude.mjs switches
//! (`fake-claude-args.txt` in the graph folder, the session's cwd): a
//! swallowed first line, a dropped Enter, and a dialog on screen. Needs `node`.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_graph_lib::contract::RunTarget;
use claude_graph_lib::contract::{NodeEvent, NodeEventKind, NodeStatus, RunState, RunStatus};
use claude_graph_lib::run_api::AppState;
use claude_graph_lib::terminal::deliver::asking;
use common::term::*;
use common::*;
use serde_json::json;

const PROMPT: &str = "hello delivery, please answer";

struct Trial {
    app: AppState,
    rec: Arc<Recorder>,
    tmp: tempfile::TempDir,
}

/// A one-node graph whose fake CLI runs with `switches`.
fn trial(switches: &str) -> Trial {
    use_fake_claude("");
    let g = graph(
        vec![node("n", 0.0, 0.0, json!({ "prompt": PROMPT }))],
        &[],
        settings(1, false),
    );
    let tmp = graph_dir(&g);
    std::fs::write(tmp.path().join("fake-claude-args.txt"), switches).unwrap();
    let app = trusted_app(tmp.path());
    Trial {
        app,
        rec: Arc::new(Recorder::default()),
        tmp,
    }
}

fn texts(rec: &Recorder) -> Vec<String> {
    rec.events_of("n", NodeEventKind::Text)
}

fn event_at(rec: &Recorder, text: &str) -> NodeEvent {
    rec.events
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.node_id == "n" && e.text == text)
        .cloned()
        .unwrap_or_else(|| panic!("no event {text:?}"))
}

/// The run succeeded, the prompt arrived once, `steps` came in this order,
/// and the node turned `running` only after the `submitted` event.
fn assert_recovered(t: &Trial, run_id: &str, state: &RunState, steps: &[&str]) {
    assert_eq!(state.status, RunStatus::Succeeded, "{state:#?}");
    assert_eq!(received(t.tmp.path(), run_id, "s1"), [PROMPT]);
    let all = texts(&t.rec);
    let at: Vec<usize> = steps
        .iter()
        .map(|s| all.iter().position(|e| e == s).unwrap_or(usize::MAX))
        .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{steps:?} in {all:?}");
    let submitted = event_at(&t.rec, "submitted").ts;
    let running = state.nodes["n"].started_at.expect("started_at");
    assert!(
        running >= submitted,
        "running {running} before submit {submitted}"
    );
    let hooks = std::fs::read_to_string(
        t.tmp
            .path()
            .join("runs")
            .join(run_id)
            .join("sessions/s1/hooks.log"),
    )
    .unwrap();
    let words: Vec<&str> = hooks.lines().map(str::trim).collect();
    assert_eq!(words, ["start", "submit", "stop"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_swallowed_first_line_is_retyped() {
    let t = trial("--fake-swallow-first");
    let (run_id, state) = run(&t.app, t.tmp.path(), RunTarget::All, &t.rec).await;
    assert_recovered(
        &t,
        &run_id,
        &state,
        &[
            "delivering message…",
            "typed",
            "retyping (cycle 2)",
            "submitted",
        ],
    );
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_enter_is_pressed_again() {
    let t = trial("--fake-drop-enter");
    let (run_id, state) = run(&t.app, t.tmp.path(), RunTarget::All, &t.rec).await;
    assert_recovered(
        &t,
        &run_id,
        &state,
        &[
            "delivering message…",
            "typed",
            "enter again (2)",
            "submitted",
        ],
    );
    assert!(!texts(&t.rec).iter().any(|e| e.starts_with("retyping")));
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dialog_keeps_the_node_queued_until_it_closes() {
    const DIALOG: Duration = Duration::from_millis(7000);
    let t = trial(&format!("--fake-dialog-ms={}", DIALOG.as_millis()));
    let t0 = Instant::now();
    let (run_id, join) = t
        .app
        .start_by_mode(t.tmp.path(), RunTarget::All, t.rec.clone())
        .unwrap();
    let notice = asking("T n");
    wait_until("the dialog notification", || {
        !t.rec.events_of("n", NodeEventKind::Notification).is_empty()
    })
    .await;
    assert_eq!(
        t.rec.events_of("n", NodeEventKind::Notification),
        std::slice::from_ref(&notice)
    );
    // While the dialog is up: queued, delivering, nothing typed.
    while t0.elapsed() < DIALOG - Duration::from_millis(1500) {
        let last = t.rec.states.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.nodes["n"].status, NodeStatus::Queued);
        assert_eq!(texts(&t.rec), ["delivering message…"]);
        assert!(received(t.tmp.path(), &run_id, "s1").is_empty());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let state = finish(join).await;
    assert!(t0.elapsed() >= DIALOG);
    assert_recovered(
        &t,
        &run_id,
        &state,
        &["delivering message…", "typed", "submitted"],
    );
    assert_eq!(
        t.rec.events_of("n", NodeEventKind::Notification),
        [notice],
        "one notification per dialog"
    );
    kill_sessions(&t.app, &run_id).await;
}

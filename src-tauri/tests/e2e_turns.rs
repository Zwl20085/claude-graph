//! Turn outcomes in terminal mode (docs/ARCHITECTURE.md "Turn outcomes")
//! with the fake CLI's interactive mode in a real pty: a question pauses
//! the session until it is answered or continued, an offer with written
//! deliverables does not, a dialog is detected, answered from the app or in
//! the window, and everything lands in questions.jsonl.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use claude_graph_lib::contract::{
    AnswerVia, DialogKind, NodeStatus, PromptAnswer, PromptSource, QuestionKind, RunState,
    RunTarget, SessionPrompt, TurnPrompt,
};
use claude_graph_lib::run_api::AppState;
use claude_graph_lib::terminal::api as term;
use claude_graph_lib::terminal::prompt_api;
use claude_graph_lib::terminal::questions;
use common::term::*;
use common::*;
use serde_json::{json, Value};
use tokio::task::JoinHandle;

struct Trial {
    tmp: tempfile::TempDir,
    app: AppState,
    rec: Arc<Recorder>,
}

fn trial(nodes: Vec<Value>, edges: &[(&str, &str)]) -> Trial {
    use_fake_claude("");
    let tmp = graph_dir(&graph(nodes, edges, settings(1, false)));
    let app = trusted_app(tmp.path());
    Trial {
        tmp,
        app,
        rec: Arc::new(Recorder::default()),
    }
}

fn start(t: &Trial) -> (String, JoinHandle<RunState>) {
    t.app
        .start_by_mode(t.tmp.path(), RunTarget::All, t.rec.clone())
        .unwrap()
}

async fn prompt_of(rec: &Recorder, node: &str, n: usize) -> SessionPrompt {
    wait_until("a session prompt", || {
        rec.session_prompts
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.node_id == node)
            .count()
            >= n
    })
    .await;
    rec.session_prompts
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.node_id == node)
        .nth(n - 1)
        .cloned()
        .unwrap()
}

fn status_of(rec: &Recorder, node: &str) -> Option<NodeStatus> {
    rec.states
        .lock()
        .unwrap()
        .last()
        .and_then(|s| s.nodes.get(node).map(|n| n.status))
}

fn node_dir(dir: &Path, run_id: &str, node: &str) -> std::path::PathBuf {
    dir.join("runs").join(run_id).join(node)
}

fn summary(dir: &Path, run_id: &str, node: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(node_dir(dir, run_id, node).join("summary.json")).unwrap(),
    )
    .unwrap()
}

fn two_nodes(first: &str) -> Vec<Value> {
    vec![
        node("a", 0.0, 0.0, json!({ "prompt": first })),
        node("b", 0.0, 100.0, json!({ "prompt": "second task" })),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_question_pauses_the_session_until_it_is_answered() {
    let t = trial(two_nodes("set up tests FAKE_QUESTION"), &[("a", "b")]);
    let dir = t.tmp.path();
    let (run_id, join) = start(&t);
    let p = prompt_of(&t.rec, "a", 1).await;
    let TurnPrompt::Reply(q) = &p.prompt else {
        panic!("reply question expected: {p:?}")
    };
    assert_eq!(q.question, "Which framework should I use?");
    let labels: Vec<&str> = q.choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["vitest", "jest"]);
    wait_until("a waiting", || {
        status_of(&t.rec, "a") == Some(NodeStatus::Waiting)
    })
    .await;
    let live = term::list_sessions(t.app.terminals(), None, &run_id).unwrap();
    assert_eq!(live[0].pending_prompt.as_deref(), Some(q.id.as_str()));
    // Paused: the next node is not typed.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(received(dir, &run_id, "s1").len(), 1);
    assert_eq!(status_of(&t.rec, "b"), Some(NodeStatus::Queued));
    // A wrong id or a text that is empty is refused.
    let terms = t.app.terminals();
    assert!(
        prompt_api::answer_prompt(terms, &run_id, "s1", "zz", PromptAnswer::Choice(0)).is_err()
    );
    prompt_api::answer_prompt(terms, &run_id, "s1", &q.id, PromptAnswer::Choice(0)).unwrap();
    let state = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    assert_eq!(state.nodes["b"].status, NodeStatus::Succeeded);
    assert_eq!(
        received(dir, &run_id, "s1"),
        ["set up tests FAKE_QUESTION", "vitest", "second task"]
    );
    let reply = fs::read_to_string(node_dir(dir, &run_id, "a").join("reply.md")).unwrap();
    assert!(
        reply.contains("> vitest\n\nThanks, using vitest."),
        "{reply}"
    );
    let records = questions::read(&node_dir(dir, &run_id, "a"));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, QuestionKind::Question);
    let answer = records[0].answer.clone().expect("answered");
    assert_eq!(
        (answer.via, answer.text.as_deref(), answer.index),
        (AnswerVia::App, Some("vitest"), Some(0))
    );
    let s = summary(dir, &run_id, "a");
    assert_eq!(
        (s["questions"].as_u64(), s["waited_for_user"].as_bool()),
        (Some(1), Some(true))
    );
    let cleared = t.rec.cleared.lock().unwrap().clone();
    assert_eq!(cleared.len(), 1);
    assert_eq!(cleared[0].via, AnswerVia::App);
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_ends_the_waiting_node_and_the_chain_goes_on() {
    let t = trial(two_nodes("plan FAKE_QUESTION"), &[("a", "b")]);
    let dir = t.tmp.path();
    let (run_id, join) = start(&t);
    let p = prompt_of(&t.rec, "a", 1).await;
    let terms = t.app.terminals();
    assert!(prompt_api::continue_node(terms, &run_id, "s1", "b").is_err());
    prompt_api::continue_node(terms, &run_id, "s1", "a").unwrap();
    let state = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    assert_eq!(state.nodes["b"].status, NodeStatus::Succeeded);
    assert_eq!(
        received(dir, &run_id, "s1"),
        ["plan FAKE_QUESTION", "second task"]
    );
    let records = questions::read(&node_dir(dir, &run_id, "a"));
    assert_eq!(records[0].id, p.prompt.id());
    assert_eq!(records[0].answer.as_ref().unwrap().via, AnswerVia::Continue);
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_offer_after_the_deliverables_does_not_pause() {
    let nodes = vec![
        node(
            "a",
            0.0,
            0.0,
            json!({ "goal": "Write it. FAKE_OFFER",
                    "deliverables": [{ "path": "out.md", "description": "the output" }] }),
        ),
        node("b", 0.0, 100.0, json!({ "prompt": "second task" })),
    ];
    let t = trial(nodes, &[("a", "b")]);
    let dir = t.tmp.path();
    let (run_id, join) = start(&t);
    let state = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    assert_eq!(state.nodes["b"].status, NodeStatus::Succeeded);
    assert!(t.rec.session_prompts.lock().unwrap().is_empty());
    let questions_seen = t
        .rec
        .events_of("a", claude_graph_lib::contract::NodeEventKind::Question);
    assert!(
        questions_seen[0].starts_with("Want me to also write tests?"),
        "{questions_seen:?}"
    );
    let records = questions::read(&node_dir(dir, &run_id, "a"));
    assert_eq!(records.len(), 1);
    assert!(records[0].offer && records[0].answer.is_none());
    let s = summary(dir, &run_id, "a");
    assert_eq!(
        (s["questions"].as_u64(), s["waited_for_user"].as_bool()),
        (Some(1), Some(false))
    );
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dialog_is_answered_from_the_app() {
    let t = trial(two_nodes("pick one FAKE_DIALOG"), &[("a", "b")]);
    let dir = t.tmp.path();
    let (run_id, join) = start(&t);
    let p = prompt_of(&t.rec, "a", 1).await;
    let TurnPrompt::Dialog(d) = &p.prompt else {
        panic!("dialog expected: {p:?}")
    };
    assert_eq!(d.kind, DialogKind::Choice);
    assert_eq!(d.question, "Which framework should I use?");
    assert_eq!(
        d.source,
        PromptSource::Transcript,
        "enriched from the transcript"
    );
    assert_eq!(d.header.as_deref(), Some("Framework"));
    let labels: Vec<&str> = d.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["vitest", "jest", "Type something."]);
    assert_eq!(d.options[1].detail.as_deref(), Some("The classic."));
    assert_eq!((d.selected, d.allows_text, d.multi), (Some(0), true, false));
    wait_until("a waiting", || {
        status_of(&t.rec, "a") == Some(NodeStatus::Waiting)
    })
    .await;
    let terms = t.app.terminals();
    prompt_api::answer_prompt(terms, &run_id, "s1", &d.id, PromptAnswer::Choice(1)).unwrap();
    let state = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    assert_eq!(state.nodes["b"].status, NodeStatus::Succeeded);
    let reply = fs::read_to_string(node_dir(dir, &run_id, "a").join("reply.md")).unwrap();
    assert!(reply.contains("You chose jest."), "{reply}");
    let records = questions::read(&node_dir(dir, &run_id, "a"));
    assert_eq!(records[0].kind, QuestionKind::Dialog);
    let answer = records[0].answer.clone().expect("answered");
    assert_eq!((answer.via, answer.index), (AnswerVia::App, Some(1)));
    assert_eq!(t.rec.cleared.lock().unwrap()[0].via, AnswerVia::App);
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dialog_answered_in_the_window_clears_and_text_answers_work() {
    let nodes = vec![
        node("a", 0.0, 0.0, json!({ "prompt": "pick FAKE_DIALOG" })),
        node("b", 0.0, 100.0, json!({ "prompt": "again FAKE_DIALOG" })),
    ];
    let t = trial(nodes, &[("a", "b")]);
    let dir = t.tmp.path();
    let (run_id, join) = start(&t);
    prompt_of(&t.rec, "a", 1).await;
    // The user answers in the window: Down, Enter.
    term::pty_write(t.app.terminals(), &run_id, "s1", "\u{1b}[B").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    term::pty_write(t.app.terminals(), &run_id, "s1", "\r").unwrap();
    // Node b: a free-text answer through the "Type something." option.
    let p = prompt_of(&t.rec, "b", 1).await;
    let terms = t.app.terminals();
    let err = prompt_api::answer_prompt(
        terms,
        &run_id,
        "s1",
        p.prompt.id(),
        PromptAnswer::Choices(vec![0]),
    );
    assert!(err.is_err(), "single choice takes no list");
    prompt_api::answer_prompt(
        terms,
        &run_id,
        "s1",
        p.prompt.id(),
        PromptAnswer::Text("mocha".into()),
    )
    .unwrap();
    let state = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    assert_eq!(state.nodes["b"].status, NodeStatus::Succeeded);
    let a = fs::read_to_string(node_dir(dir, &run_id, "a").join("reply.md")).unwrap();
    assert!(a.contains("You chose jest."), "{a}");
    let b = fs::read_to_string(node_dir(dir, &run_id, "b").join("reply.md")).unwrap();
    assert!(b.contains("You chose mocha."), "{b}");
    let cleared = t.rec.cleared.lock().unwrap().clone();
    assert_eq!(cleared[0].via, AnswerVia::Window);
    assert_eq!(cleared[1].via, AnswerVia::App);
    let records = questions::read(&node_dir(dir, &run_id, "a"));
    assert_eq!(records[0].answer.as_ref().unwrap().via, AnswerVia::Window);
    kill_sessions(&t.app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permission_prompt_names_the_tool() {
    let t = trial(
        vec![node(
            "a",
            0.0,
            0.0,
            json!({ "prompt": "do FAKE_PERMISSION" }),
        )],
        &[],
    );
    let (run_id, join) = start(&t);
    let p = prompt_of(&t.rec, "a", 1).await;
    let TurnPrompt::Dialog(d) = &p.prompt else {
        panic!("dialog expected")
    };
    assert_eq!(d.kind, DialogKind::Permission);
    assert_eq!(d.header.as_deref(), Some("Bash: mkdir fake_dir"));
    assert_eq!(d.options.len(), 3);
    assert!(!d.allows_text);
    let terms = t.app.terminals();
    let err =
        prompt_api::answer_prompt(terms, &run_id, "s1", &d.id, PromptAnswer::Text("x".into()));
    assert_eq!(err.unwrap_err().message, prompt_api::ONLY_OPTIONS);
    prompt_api::answer_prompt(terms, &run_id, "s1", &d.id, PromptAnswer::Choice(0)).unwrap();
    let state = finish(join).await;
    assert_eq!(state.nodes["a"].status, NodeStatus::Succeeded);
    kill_sessions(&t.app, &run_id).await;
}

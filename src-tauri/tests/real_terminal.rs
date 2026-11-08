//! Terminal mode against the real `claude` CLI (the owner's own login), in a
//! real pty. Ignored by default; run with
//! `cargo test --test real_terminal -- --ignored --nocapture` (about one cent
//! per run; the test makes two runs).
//!
//! Run 1, in a fresh temp folder: Claude Code asks whether to trust it (and
//! maybe whether to use a project MCP server). The test first leaves the
//! dialog alone and checks that the node stays `queued` with the dialog
//! notification for 5 s and nothing is typed; then it answers like a user in
//! the window (pty_write; never the "No, exit" choice) and checks `submit`,
//! then `stop`, then success. Run 2, in the same (now trusted) folder: no
//! dialog, straight delivery.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_graph_lib::claude_bin::ENV_CLAUDE_BIN;
use claude_graph_lib::contract::{Graph, NodeEventKind, NodeStatus, RunState, RunTarget};
use claude_graph_lib::run_api::AppState;
use claude_graph_lib::terminal::api as term;
use claude_graph_lib::terminal::screen::plain_tail;
use common::*;

const LIMIT: Duration = Duration::from_secs(240);
const HANDS_OFF: Duration = Duration::from_secs(5);

fn one_node_graph() -> Graph {
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "name": "real-terminal",
        "settings": { "max_parallel": 1, "default_permission_mode": "claude_settings" },
        "nodes": [{
            "id": "ok",
            "title": "Say ok",
            "prompt": "Reply with the single word ok and create no files",
            "model": "haiku",
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
    if !pty.output.screen().dialog_open() {
        return;
    }
    let tail = plain_tail(&pty.output.replay().text, 400).to_lowercase();
    let highlighted = tail
        .rsplit(['❯', '>'])
        .next()
        .unwrap_or("")
        .trim_start()
        .to_string();
    println!("answering dialog: …{}", plain_tail(&tail, 200));
    if highlighted.starts_with("no") {
        term::pty_write(app.terminals(), run_id, "s1", "\u{1b}[B").unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    term::pty_write(app.terminals(), run_id, "s1", "\r").unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
}

fn events(rec: &Recorder, run_id: &str, kind: NodeEventKind) -> Vec<String> {
    rec.events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.run_id == run_id && e.kind == kind)
        .map(|e| e.text.clone())
        .collect()
}

fn node_status(rec: &Recorder, run_id: &str) -> Option<NodeStatus> {
    let states = rec.states.lock().unwrap();
    let last = states.iter().rev().find(|s| s.run_id == run_id)?;
    Some(last.nodes["ok"].status)
}

/// One run; a dialog is left alone for [`HANDS_OFF`] (checked), then answered.
async fn run_once(
    app: &AppState,
    dir: &std::path::Path,
    rec: &Arc<Recorder>,
) -> (String, RunState) {
    let t0 = Instant::now();
    let (run_id, mut join) = app.start_by_mode(dir, RunTarget::All, rec.clone()).unwrap();
    let mut noticed: Option<Instant> = None;
    let state = loop {
        assert!(t0.elapsed() < LIMIT, "run did not finish in {LIMIT:?}");
        tokio::select! {
            done = &mut join => break done.unwrap(),
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
        let notes = events(rec, &run_id, NodeEventKind::Notification);
        if noticed.is_none() && !notes.is_empty() {
            println!("[{:?}] notification: {notes:?}", t0.elapsed());
            noticed = Some(Instant::now());
        }
        match noticed {
            Some(at) if at.elapsed() < HANDS_OFF => {
                assert_eq!(node_status(rec, &run_id), Some(NodeStatus::Queued));
                let steps = events(rec, &run_id, NodeEventKind::Text);
                assert!(
                    !steps.iter().any(|s| s == "typed"),
                    "typed into a dialog: {steps:?}"
                );
            }
            Some(_) => answer_dialogs(app, &run_id).await,
            None => {}
        }
    };
    (run_id, state)
}

fn report(dir: &std::path::Path, run_id: &str, rec: &Recorder, state: &RunState) -> Vec<String> {
    let session_dir = dir.join("runs").join(run_id).join("sessions").join("s1");
    let hooks =
        std::fs::read_to_string(common::term::hooks_dir(dir, run_id, "s1").join("hooks.log"))
            .unwrap_or_default();
    let transcript =
        std::fs::read_to_string(session_dir.join("transcript.log")).unwrap_or_default();
    let plain = plain_tail(&transcript, 1500);
    println!("--- run {run_id} hooks.log ---\n{hooks}--- transcript (plain tail) ---\n{plain}");
    let reply_md = dir.join("runs").join(run_id).join("ok").join("reply.md");
    let reply = std::fs::read_to_string(&reply_md).unwrap_or_default();
    println!("--- reply.md ---\n{reply}--- end reply.md ---");
    assert!(
        reply.starts_with("> Reply with the single word ok"),
        "reply.md: {reply}"
    );
    for kind in [
        NodeEventKind::Notification,
        NodeEventKind::Text,
        NodeEventKind::Stderr,
    ] {
        println!("{kind:?} events: {:?}", events(rec, run_id, kind));
    }
    println!(
        "run {run_id}: {:?}; node {:?}",
        state.status, state.nodes["ok"]
    );
    let replied_ok = plain
        .split('●')
        .skip(1)
        .any(|p| p.trim_start().to_lowercase().starts_with("ok"));
    assert!(replied_ok, "transcript has no '● ok' reply");
    hooks.lines().map(|l| l.trim().to_string()).collect()
}

fn assert_submit_then_stop(words: &[String]) {
    let at = |w: &str| words.iter().position(|x| x == w);
    let (start, submit, stop) = (at("start"), at("submit"), at("stop"));
    assert!(start.is_some(), "no start hook line");
    assert!(submit.is_some(), "no submit hook line");
    assert!(stop > submit, "stop must follow submit: {words:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real claude CLI interactively (two short replies)"]
async fn real_claude_delivery_waits_for_dialogs_and_is_confirmed() {
    std::env::remove_var(ENV_CLAUDE_BIN);
    let tmp = graph_dir(&one_node_graph());
    let dir = tmp.path();
    let app = trusted_app(dir);
    let rec = Arc::new(Recorder::default());

    let (run1, state) = run_once(&app, dir, &rec).await;
    let words = report(dir, &run1, &rec, &state);
    assert!(
        !events(&rec, &run1, NodeEventKind::Notification).is_empty(),
        "a fresh folder should show the trust dialog"
    );
    assert_submit_then_stop(&words);
    assert_eq!(state.nodes["ok"].status, NodeStatus::Succeeded);

    // Same folder, now trusted: no dialog.
    let (run2, state) = run_once(&app, dir, &rec).await;
    let words = report(dir, &run2, &rec, &state);
    assert_eq!(
        events(&rec, &run2, NodeEventKind::Notification),
        Vec::<String>::new()
    );
    assert_submit_then_stop(&words);
    assert_eq!(state.nodes["ok"].status, NodeStatus::Succeeded);

    app.cancel_run(&run2).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.terminals().live_count() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(app.terminals().live_count(), 0, "session not killed");
}

fn turns_graph() -> Graph {
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "name": "real-turns",
        "settings": { "max_parallel": 1, "default_permission_mode": "claude_settings" },
        "nodes": [
            { "id": "ask", "title": "Ask", "model": "haiku", "position": { "x": 0, "y": 0 },
              "prompt": "Use the AskUserQuestion tool to ask me whether to use A or B, then reply with my choice." },
            { "id": "open", "title": "Open", "model": "haiku", "position": { "x": 0, "y": 100 },
              "prompt": "Do not use any tool. Ask me one short open question about my favourite colour and end your reply with it." },
        ],
        "edges": [{ "id": "e1", "from": "ask", "to": "open" }],
    }))
    .expect("graph json")
}

/// Turn outcomes with the real CLI: AskUserQuestion becomes a `dialog`
/// prompt answered from the app (option 2), a reply ending with a question
/// becomes a `question` prompt, continued from the app.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real claude CLI interactively (two short haiku turns)"]
async fn real_claude_dialog_and_question_outcomes() {
    use claude_graph_lib::contract::{PromptAnswer, TurnPrompt};
    use claude_graph_lib::terminal::prompt_api;
    std::env::remove_var(ENV_CLAUDE_BIN);
    let tmp = graph_dir(&turns_graph());
    let dir = tmp.path();
    let app = trusted_app(dir);
    let rec = Arc::new(Recorder::default());
    let t0 = Instant::now();
    let (run_id, mut join) = app.start_by_mode(dir, RunTarget::All, rec.clone()).unwrap();
    let mut handled = 0;
    let state = loop {
        assert!(t0.elapsed() < LIMIT, "run did not finish in {LIMIT:?}");
        tokio::select! {
            done = &mut join => break done.unwrap(),
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
        // Start-up dialogs (trust, MCP) only: the node dialogs are the app's.
        let started = rec
            .states
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|s| s.run_id == run_id)
            .is_some_and(|s| s.nodes["ask"].status != NodeStatus::Queued);
        if !started {
            answer_dialogs(&app, &run_id).await;
        }
        let prompts = rec.session_prompts.lock().unwrap().clone();
        for p in prompts.iter().skip(handled) {
            handled += 1;
            println!(
                "[{:?}] prompt on {}: {}",
                t0.elapsed(),
                p.node_id,
                serde_json::to_string(&p.prompt).unwrap()
            );
            tokio::time::sleep(Duration::from_millis(800)).await;
            let terms = app.terminals();
            let sent = match &p.prompt {
                TurnPrompt::Dialog(d) => {
                    let b = d.options.iter().position(|o| o.label == "B").unwrap_or(1);
                    prompt_api::answer_prompt(
                        terms,
                        &run_id,
                        &p.sid,
                        &d.id,
                        PromptAnswer::Choice(b),
                    )
                }
                TurnPrompt::Reply(_) => {
                    prompt_api::continue_node(terms, &run_id, &p.sid, &p.node_id)
                }
            };
            println!("  answered: {sent:?}");
        }
    };
    for id in ["ask", "open"] {
        let reply =
            std::fs::read_to_string(dir.join("runs").join(&run_id).join(id).join("reply.md"))
                .unwrap_or_default();
        let questions = std::fs::read_to_string(
            dir.join("runs")
                .join(&run_id)
                .join(id)
                .join("questions.jsonl"),
        )
        .unwrap_or_default();
        println!("--- {id} reply.md ---\n{reply}--- {id} questions.jsonl ---\n{questions}");
    }
    let hooks =
        std::fs::read_to_string(common::term::hooks_dir(dir, &run_id, "s1").join("hooks.log"))
            .unwrap_or_default();
    println!(
        "--- hooks.log (words) ---\n{}",
        hooks
            .lines()
            .map(|l| l.split_whitespace().next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!(
        "run {run_id}: {:?}; nodes {:?}",
        state.status,
        state
            .nodes
            .iter()
            .map(|(k, v)| (k.clone(), v.status))
            .collect::<Vec<_>>()
    );
    let prompts = rec.session_prompts.lock().unwrap().clone();
    assert!(
        prompts.iter().any(|p| p.node_id == "ask" && matches!(&p.prompt, TurnPrompt::Dialog(d) if d.options.iter().any(|o| o.label == "B"))),
        "no AskUserQuestion dialog: {prompts:?}"
    );
    assert!(
        prompts
            .iter()
            .any(|p| p.node_id == "open" && matches!(p.prompt, TurnPrompt::Reply(_))),
        "no reply question: {prompts:?}"
    );
    assert_eq!(state.nodes["ask"].status, NodeStatus::Succeeded);
    assert_eq!(state.nodes["open"].status, NodeStatus::Succeeded);
    let ask = std::fs::read_to_string(dir.join("runs").join(&run_id).join("ask/reply.md")).unwrap();
    assert!(ask.contains('B'), "{ask}");
    app.cancel_run(&run_id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.terminals().live_count() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

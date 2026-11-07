//! End-to-end: assistant mode through scripts/fake-claude.mjs in scheduler
//! mode (needs `node`). Every turn also leaves a grandchild holding stdout for
//! a moment (`--hold-stdout-ms`), so the runner's drain and tree kill are used.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_graph_lib::assistant::journal::{self, assistant_dir, turn_dir};
use claude_graph_lib::assistant::turn::INSTRUCTION;
use claude_graph_lib::contract::{
    AssistantKind, AssistantRole, AssistantSession, AssistantState, AssistantStatus, NodeEventKind,
};
use claude_graph_lib::run_api::AppState;
use common::*;

const FAKE_ARGS: &str = "--hold-stdout-ms 300";

async fn wait(app: &AppState, dir: &Path) -> AssistantState {
    tokio::time::timeout(TIMEOUT, app.assistant_wait(dir))
        .await
        .expect("turn finished in time");
    app.assistant_state(dir).unwrap()
}

fn kinds(state: &AssistantState) -> Vec<(AssistantRole, AssistantKind)> {
    state.messages.iter().map(|m| (m.role, m.kind)).collect()
}

fn spawn_lines(recorder: &Recorder) -> Vec<String> {
    recorder
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.run_id == "assistant" && e.node_id == "scheduler")
        .filter(|e| e.kind == NodeEventKind::System && e.text.starts_with("spawn: "))
        .map(|e| e.text.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn instruction_question_answer_final_and_reset() {
    use_fake_claude(FAKE_ARGS);
    let tmp = graph_dir(&example_graph());
    let dir = tmp.path();
    let app = fresh_app();
    let recorder = Arc::new(Recorder::default());

    let empty = app.assistant_state(dir).unwrap();
    assert_eq!(empty.session.status, AssistantStatus::Idle);
    assert!(empty.messages.is_empty());
    assert!(!assistant_dir(dir).exists());
    let err = app
        .assistant_send(dir, "  \n", recorder.clone())
        .unwrap_err();
    assert_eq!(err.code, "invalid_input");
    assert_eq!(app.assistant_cancel(dir).unwrap_err().code, "not_found");

    // ---- Turn 1: instruction -> working -> waiting ----
    let started = app
        .assistant_send(
            dir,
            "  Execute the workflow from the start.  ",
            recorder.clone(),
        )
        .unwrap();
    assert_eq!(started.session.status, AssistantStatus::Working);
    assert_eq!(started.session.turns, 1);
    assert_eq!(
        kinds(&started),
        vec![(AssistantRole::User, AssistantKind::Instruction)]
    );
    assert_eq!(
        started.messages[0].text,
        "Execute the workflow from the start."
    );
    assert_eq!(app.assistant_active_count(), 1);
    let busy = app
        .assistant_send(dir, "again", recorder.clone())
        .unwrap_err();
    assert_eq!(busy.code, "assistant_busy");
    assert_eq!(
        app.assistant_reset(dir, recorder.clone()).unwrap_err().code,
        "assistant_busy"
    );

    let after = wait(&app, dir).await;
    assert_eq!(app.assistant_active_count(), 0);
    assert_eq!(
        after.session.status,
        AssistantStatus::Waiting,
        "{:?}",
        after.session.last_error
    );
    assert_eq!(
        kinds(&after),
        vec![
            (AssistantRole::User, AssistantKind::Instruction),
            (AssistantRole::Scheduler, AssistantKind::Note),
            (AssistantRole::Scheduler, AssistantKind::Stage),
            (AssistantRole::Scheduler, AssistantKind::Decision),
            (AssistantRole::Scheduler, AssistantKind::Question),
        ]
    );
    let stage = &after.messages[2];
    assert_eq!(
        stage.title.as_deref(),
        Some("survey"),
        "first node in topo order"
    );
    let decision = &after.messages[3];
    assert_eq!(decision.title.as_deref(), Some("Order"));
    assert!(decision
        .text
        .contains("### detail\nkept in the decision body"));
    assert!(after.messages.iter().all(|m| m.turn == 1));
    let sid = after.session.session_id.clone().expect("session id stored");
    assert_eq!(
        after.session.model.as_deref(),
        Some("claude-fake-scheduler")
    );
    assert!(after
        .session
        .workflow_hash
        .as_deref()
        .is_some_and(|h| h.len() == 64));

    // Files of the turn and of the scheduler.
    let t1 = turn_dir(dir, 1);
    for f in ["prompt.md", "events.jsonl", "reply.md"] {
        assert!(t1.join(f).is_file(), "turns/0001/{f} missing");
    }
    let prompt1 = fs::read_to_string(t1.join("prompt.md")).unwrap();
    assert!(prompt1.starts_with("# Work scheduler charter"));
    assert!(prompt1.contains("## Workflow\n\nGlobal limit: max_parallel = 2"));
    assert!(prompt1.contains("### Node `design`: Layout rule set"));
    assert!(prompt1.ends_with("## Instruction\n\nExecute the workflow from the start.\n"));
    assert!(assistant_dir(dir).join("CHARTER.md").is_file());
    assert!(assistant_dir(dir).join("PLAN.md").is_file());

    // The command line carries no node flags.
    let spawns = spawn_lines(&recorder);
    println!("scheduler {}", spawns[0]);
    assert!(spawns[0].contains(INSTRUCTION));
    assert!(spawns[0].contains("--output-format stream-json --verbose --add-dir"));
    for flag in [
        "--model",
        "--permission-mode",
        "--allowedTools",
        "--max-turns",
        "--resume",
    ] {
        assert!(!spawns[0].contains(flag), "{flag} in {}", spawns[0]);
    }
    let tool_uses: Vec<String> = recorder.events_of("scheduler", NodeEventKind::ToolUse);
    assert!(tool_uses.iter().any(|t| t.starts_with("Write")));

    // ---- Turn 2: answer -> done, resumed session, workflow not resent ----
    let answering = app
        .assistant_send(dir, "Yes, go on. FAKE_FINAL", recorder.clone())
        .unwrap();
    assert_eq!(
        answering.messages.last().unwrap().kind,
        AssistantKind::Answer
    );
    let done = wait(&app, dir).await;
    assert_eq!(done.session.status, AssistantStatus::Done);
    assert_eq!(done.session.turns, 2);
    assert_eq!(done.session.session_id.as_deref(), Some(sid.as_str()));
    let last_two: Vec<_> = kinds(&done).into_iter().rev().take(2).collect();
    assert_eq!(
        last_two,
        vec![
            (AssistantRole::Scheduler, AssistantKind::Final),
            (AssistantRole::Scheduler, AssistantKind::Stage),
        ]
    );
    assert_eq!(
        fs::read_to_string(turn_dir(dir, 2).join("prompt.md")).unwrap(),
        "## Message from the assistant\n\nYes, go on. FAKE_FINAL\n"
    );
    let spawns = spawn_lines(&recorder);
    assert!(
        spawns[1].contains(&format!("--resume {sid}")),
        "{}",
        spawns[1]
    );

    // ---- Reset -> idle, session forgotten, journal kept ----
    let reset = app.assistant_reset(dir, recorder.clone()).unwrap();
    assert_eq!(reset.session.status, AssistantStatus::Idle);
    assert_eq!(reset.session.session_id, None);
    assert_eq!(reset.session.workflow_hash, None);
    assert_eq!(reset.session.turns, 2);
    let note = reset.messages.last().unwrap();
    assert_eq!(
        (note.role, note.kind, note.turn),
        (AssistantRole::App, AssistantKind::System, 0)
    );
    assert!(note.text.starts_with("Session reset"));
    assert_eq!(reset.messages.len(), done.messages.len() + 1);
    assert_eq!(app.assistant_state(dir).unwrap(), reset);
    assert!(turn_dir(dir, 2).join("reply.md").is_file());

    // Every change was announced; the last update is the reset state.
    let updates = recorder.assistant.lock().unwrap();
    assert!(updates.len() >= 5, "{} updates", updates.len());
    assert_eq!(updates.last().unwrap(), &reset);
    assert!(updates
        .iter()
        .any(|u| u.session.status == AssistantStatus::Waiting));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_kills_the_turn_and_shutdown_cancels_too() {
    use_fake_claude(FAKE_ARGS);
    let tmp = graph_dir(&example_graph());
    let dir = tmp.path();
    let app = fresh_app();
    let recorder = Arc::new(Recorder::default());

    let begun = Instant::now();
    app.assistant_send(dir, "Work slowly FAKE_SLEEP_MS=20000", recorder.clone())
        .unwrap();
    // Let the process start and write its plan before killing it.
    let plan = assistant_dir(dir).join("PLAN.md");
    while !plan.is_file() && begun.elapsed() < TIMEOUT {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    app.assistant_cancel(dir).unwrap();
    let state = wait(&app, dir).await;
    assert!(
        begun.elapsed() < Duration::from_secs(15),
        "{:?}",
        begun.elapsed()
    );
    assert_eq!(state.session.status, AssistantStatus::Cancelled);
    let last = state.messages.last().unwrap();
    assert_eq!(
        (last.role, last.kind),
        (AssistantRole::App, AssistantKind::System)
    );
    assert_eq!(last.text, "Turn 1 cancelled");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !assistant_dir(dir).join("slept.txt").exists(),
        "the scheduler process was killed before it finished sleeping"
    );
    assert_eq!(app.assistant_cancel(dir).unwrap_err().code, "not_found");

    // App exit cancels a running turn the same way.
    app.assistant_send(dir, "Again FAKE_SLEEP_MS=20000", recorder.clone())
        .unwrap();
    assert_eq!(app.shutdown(Duration::from_secs(10)).await, 0);
    let state = app.assistant_state(dir).unwrap();
    assert_eq!(state.session.status, AssistantStatus::Cancelled);
    assert_eq!(state.session.turns, 2);
    assert_eq!(state.messages.last().unwrap().text, "Turn 2 cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_denials_and_stale_sweep() {
    use_fake_claude(FAKE_ARGS);
    let tmp = graph_dir(&example_graph());
    let dir = tmp.path();
    let app = fresh_app();
    let recorder = Arc::new(Recorder::default());

    app.assistant_send(dir, "Break FAKE_ERROR", recorder.clone())
        .unwrap();
    let failed = wait(&app, dir).await;
    assert_eq!(failed.session.status, AssistantStatus::Failed);
    assert!(failed
        .session
        .last_error
        .as_deref()
        .is_some_and(|e| e.contains("fake scheduler error")));
    let last = failed.messages.last().unwrap();
    assert_eq!(
        (last.role, last.kind),
        (AssistantRole::App, AssistantKind::Error)
    );

    app.assistant_send(dir, "Build FAKE_DENY", recorder.clone())
        .unwrap();
    let denied = wait(&app, dir).await;
    assert_eq!(denied.session.status, AssistantStatus::Waiting);
    let last = denied.messages.last().unwrap();
    assert_eq!(last.kind, AssistantKind::Error);
    assert!(
        last.text.contains("denied these tools: Bash"),
        "{}",
        last.text
    );
    assert!(last.text.contains("permissions.defaultMode"));

    // A session left `working` by a crash is swept on the next read.
    let stale = AssistantSession {
        status: AssistantStatus::Working,
        ..denied.session.clone()
    };
    journal::write_session(dir, &stale).unwrap();
    let swept = app.assistant_state(dir).unwrap();
    assert_eq!(swept.session.status, AssistantStatus::Failed);
    assert_eq!(swept.session.last_error.as_deref(), Some("interrupted"));
    let last = swept.messages.last().unwrap();
    assert_eq!(
        (last.role, last.kind),
        (AssistantRole::App, AssistantKind::Error)
    );
    assert!(last.text.contains("interrupted"));
}

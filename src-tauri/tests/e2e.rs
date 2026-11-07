//! End-to-end: the example graph through scripts/fake-claude.mjs (needs `node`).

mod common;

use std::fs;
use std::sync::Arc;

use claude_graph_lib::contract::{MessageRole, NodeEventKind, NodeStatus, RunStatus, RunTarget};
use claude_graph_lib::run_api::AppState;
use claude_graph_lib::run_store::{self, LOG_HEADER};
use common::*;

const FILES: [&str; 4] = ["prompt.md", "events.jsonl", "result.md", "summary.json"];

#[tokio::test(flavor = "multi_thread")]
async fn example_graph_runs_end_to_end_and_takes_a_follow_up() {
    use_fake_claude("");
    let tmp = graph_dir(&example_graph());
    let dir = tmp.path();
    let app = AppState::default();
    let recorder = Arc::new(Recorder::default());

    let (run_id, join) = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap();
    assert!(app.is_active(&run_id));
    let err = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap_err();
    assert_eq!(err.code, "run_active");
    let final_state = finish(join).await;
    assert!(!app.is_active(&run_id));

    // run.json: all three succeeded.
    let state = run_store::read_run_state(dir, &run_id).unwrap();
    assert_eq!(state, final_state);
    assert_eq!(state.status, RunStatus::Succeeded);
    for (id, node) in &state.nodes {
        assert_eq!(
            node.status,
            NodeStatus::Succeeded,
            "{id}: {:?}",
            node.reason
        );
        assert!(node.session_id.is_some());
        assert_eq!(node.cost_usd, Some(0.0123));
    }
    assert_eq!(state.nodes.len(), 3);

    // Per-node files and deliverable stubs.
    let run_dir = dir.join("runs").join(&run_id);
    for id in ["survey", "design", "report"] {
        for f in FILES {
            assert!(run_dir.join(id).join(f).is_file(), "{id}/{f} missing");
        }
    }
    for (id, file) in [
        ("survey", "survey.md"),
        ("design", "rules.md"),
        ("design", "check_rules.py"),
        ("report", "report.md"),
    ] {
        let path = dir.join("nodes").join(id).join("deliverables").join(file);
        assert!(path.is_file(), "{} missing", path.display());
    }
    // design's brief carries survey's result and absolute deliverable path.
    let prompt = fs::read_to_string(run_dir.join("design").join("prompt.md")).unwrap();
    assert!(prompt.contains("### Literature survey (id: survey) — status: succeeded"));
    assert!(prompt.contains("/nodes/survey/deliverables/survey.md — Markdown table"));
    assert!(prompt.contains("All deliverables written."));

    // GRAPH_LOG: header + three lines.
    let log = fs::read_to_string(dir.join("GRAPH_LOG.md")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines[0], LOG_HEADER.trim_end());
    assert_eq!(lines.len(), 4, "{log}");
    assert!(lines[1..]
        .iter()
        .all(|l| l.contains("\tsucceeded\t0.0123\t")));

    // Ordering: design starts only after survey finished; report after design.
    let at = |id: &str| &state.nodes[id];
    assert!(at("design").started_at.unwrap() >= at("survey").finished_at.unwrap());
    assert!(at("report").started_at.unwrap() >= at("design").finished_at.unwrap());

    // Events reached the emitter; the command line is printed for the record.
    let spawn = recorder.events_of("design", NodeEventKind::System);
    println!("design {}", spawn.first().unwrap());
    assert!(spawn[0].contains("--output-format stream-json --verbose --model opus"));
    assert!(!recorder
        .events_of("survey", NodeEventKind::ToolUse)
        .is_empty());
    assert!(!recorder
        .events_of("survey", NodeEventKind::Result)
        .is_empty());
    let last = recorder.states.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.status, RunStatus::Succeeded);

    // list_runs view.
    let runs = run_store::list_run_states(dir).unwrap();
    let summary = run_store::summarize_run(&runs[0]);
    assert_eq!(summary.counts.succeeded, 3);
    assert!((summary.total_cost_usd.unwrap() - 0.0369).abs() < 1e-9);

    // ---- Follow-up message to a finished node of a finished run ----
    let err = app
        .send_message(dir, &run_id, "nope", "hi", recorder.clone())
        .await
        .unwrap_err();
    assert_eq!(err.code, "not_found");
    let join = app
        .send_message(
            dir,
            &run_id,
            "survey",
            "Add a column for page count.",
            recorder.clone(),
        )
        .await
        .unwrap()
        .expect("a finished run is resumed for the follow-up");
    let after = finish(join).await;
    assert_eq!(after.status, RunStatus::Succeeded);
    let survey = &after.nodes["survey"];
    assert_eq!(survey.status, NodeStatus::Succeeded);
    assert!((survey.cost_usd.unwrap() - 0.0246).abs() < 1e-9);
    assert_eq!(
        survey.session_id, state.nodes["survey"].session_id,
        "resumed session"
    );

    let messages = run_store::list_messages(dir, &run_id, "survey").unwrap();
    let roles: Vec<MessageRole> = messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::Assistant
        ]
    );
    assert_eq!(messages[1].text, "Add a column for page count.");
    let result = fs::read_to_string(run_dir.join("survey").join("result.md")).unwrap();
    assert!(result.contains("\n\n---\n## Follow-up 1 ("), "{result}");
    let summary = run_store::read_summary(dir, &run_id, "survey")
        .unwrap()
        .unwrap();
    assert!((summary.cost_usd.unwrap() - 0.0246).abs() < 1e-9);
    let events = fs::read_to_string(run_dir.join("survey").join("events.jsonl")).unwrap();
    assert_eq!(
        events
            .lines()
            .filter(|l| l.contains("\"type\":\"result\""))
            .count(),
        2
    );
    let log = fs::read_to_string(dir.join("GRAPH_LOG.md")).unwrap();
    assert!(log.lines().last().unwrap().contains("\tsurvey\tfollowup\t"));
    assert_eq!(
        recorder.events_of("survey", NodeEventKind::UserMessage),
        vec!["Add a column for page count.".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn blocked_node_fails_and_its_dependent_is_skipped() {
    use_fake_claude("");
    let mut graph = example_graph();
    let design = graph.nodes.iter_mut().find(|n| n.id == "design").unwrap();
    design.extra_context = "FAKE_FAIL please".into();
    let tmp = graph_dir(&graph);
    let dir = tmp.path();
    let app = AppState::default();
    let recorder = Arc::new(Recorder::default());

    let (run_id, join) = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap();
    let state = finish(join).await;
    assert_eq!(state.status, RunStatus::Failed);
    assert_eq!(state.nodes["survey"].status, NodeStatus::Succeeded);
    let design = &state.nodes["design"];
    assert_eq!(design.status, NodeStatus::Failed);
    assert!(
        design.reason.as_deref().unwrap().contains("BLOCKED"),
        "{design:?}"
    );
    let report = &state.nodes["report"];
    assert_eq!(report.status, NodeStatus::Skipped);
    assert_eq!(report.reason.as_deref(), Some("upstream design failed"));
    assert!(report.started_at.is_none());
    assert!(!dir.join("runs").join(&run_id).join("report").exists());

    let summary = run_store::read_summary(dir, &run_id, "design")
        .unwrap()
        .unwrap();
    assert_eq!(summary.status, NodeStatus::Failed);
    let log = fs::read_to_string(dir.join("GRAPH_LOG.md")).unwrap();
    assert!(log.contains("\tdesign\tfailed\t"));
    assert!(log.contains("\treport\tskipped\t-\t-"));

    // Rerun the failed part: survey's earlier success feeds design.
    let err = app
        .send_message(dir, &run_id, "report", "hello", recorder.clone())
        .await
        .unwrap_err();
    assert_eq!(err.message, "node has not run yet");
    let (rerun, join) = app
        .start_run(dir, RunTarget::FailedOf(run_id.clone()), recorder.clone())
        .unwrap();
    let again = finish(join).await;
    assert_ne!(rerun, run_id);
    assert_eq!(
        again.nodes.keys().collect::<Vec<_>>(),
        vec!["design", "report"]
    );
    let prompt = fs::read_to_string(
        dir.join("runs")
            .join(&rerun)
            .join("design")
            .join("prompt.md"),
    )
    .unwrap();
    assert!(prompt.contains("(id: survey) — status: succeeded"));
    assert!(prompt.contains("All deliverables written."));
}

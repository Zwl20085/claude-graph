//! End-to-end: a message sent to a running node is queued, drained as a
//! follow-up turn before the node counts as finished, and its dependent waits.
//! Separate test binary because it slows the fake CLI through the env command line.

mod common;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use claude_graph_lib::contract::{MessageRole, NodeEventKind, NodeStatus, RunStatus, RunTarget};
use claude_graph_lib::run_store;
use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn message_to_running_node_is_drained_before_dependents_start() {
    use_fake_claude("--fake-delay-ms 150");
    let mut graph = example_graph();
    graph.nodes.retain(|n| n.id != "report");
    graph.edges.retain(|e| e.to != "report");
    let tmp = graph_dir(&graph);
    let dir = tmp.path();
    let app = trusted_app(dir);
    let recorder = Arc::new(Recorder::default());

    let (run_id, join) = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap();
    // Wait until survey has produced output, so it is surely running.
    let mut waited = Duration::ZERO;
    while recorder.events_of("survey", NodeEventKind::Text).is_empty() {
        assert!(waited < TIMEOUT, "survey never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
        waited += Duration::from_millis(20);
    }
    let resumed = app
        .send_message(
            dir,
            &run_id,
            "survey",
            "Also list the venues.",
            recorder.clone(),
        )
        .await
        .unwrap();
    assert!(resumed.is_none(), "the active loop took the message");
    assert_eq!(
        recorder.events_of("survey", NodeEventKind::UserMessage),
        vec!["Also list the venues.".to_string()]
    );

    let state = finish(join).await;
    assert_eq!(state.status, RunStatus::Succeeded);
    let survey = &state.nodes["survey"];
    let design = &state.nodes["design"];
    assert_eq!(survey.status, NodeStatus::Succeeded);
    assert!((survey.cost_usd.unwrap() - 0.0246).abs() < 1e-9);
    assert!(
        design.started_at.unwrap() >= survey.finished_at.unwrap(),
        "design started before survey's follow-up finished"
    );

    // While the follow-up ran, survey was never reported finished.
    let states = recorder.states.lock().unwrap().clone();
    let first_design_start = states
        .iter()
        .position(|s| s.nodes["design"].status == NodeStatus::Running)
        .unwrap();
    let survey_results = |upto: usize| {
        states[..upto]
            .iter()
            .filter(|s| s.nodes["survey"].status == NodeStatus::Succeeded)
            .count()
    };
    assert!(survey_results(first_design_start) <= 1);

    let run_dir = dir.join("runs").join(&run_id).join("survey");
    let result = fs::read_to_string(run_dir.join("result.md")).unwrap();
    assert!(result.contains("## Follow-up 1 ("));
    let events = fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    assert_eq!(
        events
            .lines()
            .filter(|l| l.contains("\"type\":\"result\""))
            .count(),
        2
    );
    let roles: Vec<MessageRole> = run_store::list_messages(dir, &run_id, "survey")
        .unwrap()
        .iter()
        .map(|m| m.role)
        .collect();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Assistant
        ]
    );
    let log = fs::read_to_string(dir.join("GRAPH_LOG.md")).unwrap();
    assert!(log.contains("\tsurvey\tfollowup\t"));
    assert!(log.contains("\tdesign\tsucceeded\t"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_run_kills_running_nodes() {
    use_fake_claude("--fake-delay-ms 150");
    let tmp = graph_dir(&example_graph());
    let dir = tmp.path();
    let app = trusted_app(dir);
    let recorder = Arc::new(Recorder::default());
    let (run_id, join) = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap();
    let mut waited = Duration::ZERO;
    while recorder.events_of("survey", NodeEventKind::Text).is_empty() {
        assert!(waited < TIMEOUT, "survey never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
        waited += Duration::from_millis(20);
    }
    app.cancel_run(&run_id).unwrap();
    let state = finish(join).await;
    assert_eq!(state.status, RunStatus::Cancelled);
    assert_eq!(state.nodes["survey"].status, NodeStatus::Cancelled);
    for id in ["design", "report"] {
        assert!(
            matches!(
                state.nodes[id].status,
                NodeStatus::Cancelled | NodeStatus::Skipped
            ),
            "{id}: {:?}",
            state.nodes[id]
        );
    }
    assert_eq!(app.cancel_run(&run_id).unwrap_err().code, "not_found");
}

/// A follow-up to an old run must not start a second loop on a graph that
/// already has an active run.
#[tokio::test(flavor = "multi_thread")]
async fn follow_up_to_old_run_is_refused_while_another_run_is_active() {
    use_fake_claude("--fake-delay-ms 150");
    let mut graph = example_graph();
    graph.nodes.retain(|n| n.id == "survey");
    graph.edges.clear();
    let tmp = graph_dir(&graph);
    let dir = tmp.path();
    let app = trusted_app(dir);
    let recorder = Arc::new(Recorder::default());
    let (old, join) = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap();
    assert_eq!(finish(join).await.status, RunStatus::Succeeded);
    let (current, join) = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap();
    let err = app
        .send_message(dir, &old, "survey", "more please", recorder.clone())
        .await
        .unwrap_err();
    assert_eq!(err.code, "run_active");
    app.cancel_run(&current).unwrap();
    finish(join).await;
    assert!(run_store::list_messages(dir, &old, "survey")
        .unwrap()
        .iter()
        .all(|m| m.text != "more please"));
}

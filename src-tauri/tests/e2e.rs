//! End-to-end: the example graph through scripts/fake-claude.mjs (needs `node`).

mod common;

use std::fs;
use std::sync::Arc;

use claude_graph_lib::contract::{
    MessageRole, NodeEventKind, NodeRunState, NodeStatus, RunMode, RunState, RunStatus, RunTarget,
};
use claude_graph_lib::run_store::{self, LOG_HEADER};
use common::*;

const FILES: [&str; 4] = ["prompt.md", "events.jsonl", "result.md", "summary.json"];

#[tokio::test(flavor = "multi_thread")]
async fn example_graph_runs_end_to_end_and_takes_a_follow_up() {
    use_fake_claude("");
    let tmp = graph_dir(&example_graph());
    let dir = tmp.path();
    let app = trusted_app(dir);
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
async fn blocked_node_skips_dependents_and_a_done_follow_up_promotes_it() {
    use_fake_claude("");
    let mut graph = example_graph();
    let design = graph.nodes.iter_mut().find(|n| n.id == "design").unwrap();
    design.extra_context = "FAKE_BLOCKED FAKE_DENY please".into();
    let tmp = graph_dir(&graph);
    let dir = tmp.path();
    let app = trusted_app(dir);
    let recorder = Arc::new(Recorder::default());

    let (run_id, join) = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap();
    let state = finish(join).await;
    assert_eq!(state.status, RunStatus::Failed, "blocked fails the run");
    assert_eq!(state.nodes["survey"].status, NodeStatus::Succeeded);
    let design = &state.nodes["design"];
    assert_eq!(design.status, NodeStatus::Blocked, "{design:?}");
    let reason = design.reason.as_deref().unwrap();
    assert!(reason.starts_with("FAKE_BLOCKED was present"), "{reason}");
    assert!(reason.contains("May I continue without web access?"));
    assert!(!reason.contains("agent reported"));
    assert_eq!(design.permission_denials, vec!["WebSearch".to_string()]);
    let report = &state.nodes["report"];
    assert_eq!(report.status, NodeStatus::Skipped);
    assert_eq!(report.reason.as_deref(), Some("upstream design blocked"));
    assert!(report.started_at.is_none());

    // The live denial reached the emitter with the tool and its input.
    assert_eq!(
        recorder.events_of("design", NodeEventKind::PermissionDenied),
        vec![r#"WebSearch: "fake denied search""#.to_string()]
    );
    let summary = run_store::read_summary(dir, &run_id, "design")
        .unwrap()
        .unwrap();
    assert_eq!(summary.status, NodeStatus::Blocked);
    assert_eq!(summary.permission_denials, vec!["WebSearch".to_string()]);
    let log = fs::read_to_string(dir.join("GRAPH_LOG.md")).unwrap();
    assert!(log.contains("\tdesign\tblocked\t"), "{log}");
    assert!(log.contains("\treport\tskipped\t-\t-"));
    let counts = run_store::summarize_run(&state).counts;
    assert_eq!((counts.blocked, counts.failed), (1, 0));

    // "Rerun failed" targets the blocked node and its dependents.
    let (rerun, join) = app
        .start_run(dir, RunTarget::FailedOf(run_id.clone()), recorder.clone())
        .unwrap();
    let again = finish(join).await;
    assert_eq!(
        again.nodes.keys().collect::<Vec<_>>(),
        vec!["design", "report"]
    );
    assert_eq!(again.nodes["design"].status, NodeStatus::Blocked);
    assert_ne!(rerun, run_id);

    // Answering the blocked node in the first run: a DONE reply promotes it
    // (its deliverables were written by the first turn); denials are kept.
    let join = app
        .send_message(dir, &run_id, "design", "Yes, go on.", recorder.clone())
        .await
        .unwrap()
        .expect("a finished run is resumed for the follow-up");
    let after = finish(join).await;
    let design = &after.nodes["design"];
    assert_eq!(design.status, NodeStatus::Succeeded, "{design:?}");
    assert_eq!(design.reason, None);
    assert_eq!(design.permission_denials, vec!["WebSearch".to_string()]);
    assert_eq!(
        after.nodes["report"].status,
        NodeStatus::Skipped,
        "skips stay"
    );
    let summary = run_store::read_summary(dir, &run_id, "design")
        .unwrap()
        .unwrap();
    assert_eq!(summary.status, NodeStatus::Succeeded);
    assert_eq!(summary.permission_denials, vec!["WebSearch".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_node_fails_and_its_dependent_is_skipped() {
    use_fake_claude("");
    let mut graph = example_graph();
    let design = graph.nodes.iter_mut().find(|n| n.id == "design").unwrap();
    design.extra_context = "FAKE_FAIL please".into();
    let tmp = graph_dir(&graph);
    let dir = tmp.path();
    let app = trusted_app(dir);
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
        design.reason.as_deref().unwrap().contains("FAKE_FAIL"),
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

#[tokio::test(flavor = "multi_thread")]
async fn untrusted_graph_is_refused_until_trusted() {
    use_fake_claude("");
    let tmp = graph_dir(&example_graph()); // design allows Bash(python:*)
    let dir = tmp.path();
    let app = fresh_app();
    let recorder = Arc::new(Recorder::default());
    let status = app
        .trust_status(&claude_graph_lib::run_api::abs(dir))
        .unwrap();
    assert!(!status.trusted);
    assert!(status
        .items
        .iter()
        .any(|i| i.node_id.as_deref() == Some("design")));
    let err = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap_err();
    assert_eq!(err.code, "untrusted_graph");
    let err = app
        .send_message(dir, "20261107T101500Z", "survey", "hi", recorder.clone())
        .await
        .unwrap_err();
    assert_eq!(err.code, "untrusted_graph");
    assert!(!dir.join("runs").exists(), "nothing ran");

    app.trust_graph(&claude_graph_lib::run_api::abs(dir))
        .unwrap();
    let (_, join) = app.start_run(dir, RunTarget::All, recorder).unwrap();
    assert_eq!(finish(join).await.status, RunStatus::Succeeded);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_start_run_admits_exactly_one() {
    use_fake_claude("");
    let tmp = graph_dir(&example_graph());
    let dir = tmp.path().to_path_buf();
    let app = trusted_app(&dir);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let starts: Vec<_> = (0..2)
        .map(|_| {
            let (app, dir, barrier) = (app.clone(), dir.clone(), barrier.clone());
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                app.start_run(&dir, RunTarget::All, Arc::new(Recorder::default()))
            })
        })
        .collect();
    let mut joins = Vec::new();
    let mut refused = 0;
    for s in starts {
        match s.await.unwrap() {
            Ok((_, join)) => joins.push(join),
            Err(e) => {
                assert_eq!(e.code, "run_active");
                refused += 1;
            }
        }
    }
    assert_eq!((joins.len(), refused), (1, 1));
    for j in joins {
        assert_eq!(finish(j).await.status, RunStatus::Succeeded);
    }
    assert_eq!(run_store::list_run_states(&dir).unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_running_runs_are_swept_but_live_ones_are_not() {
    use_fake_claude("");
    let tmp = graph_dir(&example_graph());
    let dir = tmp.path();
    let app = trusted_app(dir);
    // A run left "running" by a crash.
    let stale_id = "20200101T000000Z";
    fs::create_dir_all(dir.join("runs").join(stale_id)).unwrap();
    let running = |status| NodeRunState {
        status,
        ..NodeRunState::pending()
    };
    let stale = RunState {
        run_id: stale_id.into(),
        graph_dir: dir.display().to_string(),
        started_at: "2020-01-01T00:00:00Z".parse().unwrap(),
        finished_at: None,
        status: RunStatus::Running,
        nodes: [
            ("survey".to_string(), running(NodeStatus::Succeeded)),
            ("design".to_string(), running(NodeStatus::Running)),
            ("report".to_string(), running(NodeStatus::Pending)),
        ]
        .into_iter()
        .collect(),
        mode: RunMode::Headless,
        sessions: Vec::new(),
    };
    run_store::write_run_state(dir, &stale).unwrap();

    let (live_id, join) = app
        .start_run(dir, RunTarget::All, Arc::new(Recorder::default()))
        .unwrap();
    let swept = app.sweep_stale(dir).unwrap();
    assert_eq!(swept, vec![stale_id.to_string()]);
    let after = run_store::read_run_state(dir, stale_id).unwrap();
    assert_eq!(after.status, RunStatus::Cancelled);
    assert_eq!(after.nodes["survey"].status, NodeStatus::Succeeded);
    for id in ["design", "report"] {
        assert_eq!(after.nodes[id].status, NodeStatus::Cancelled);
        assert_eq!(after.nodes[id].reason.as_deref(), Some("interrupted"));
    }
    let done = finish(join).await;
    assert_eq!(
        done.status,
        RunStatus::Succeeded,
        "the live run was not swept"
    );
    assert_eq!(
        run_store::read_run_state(dir, &live_id).unwrap().status,
        RunStatus::Succeeded
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn follow_up_without_status_keeps_a_succeeded_node_but_blocked_blocks_it() {
    use_fake_claude("");
    let tmp = graph_dir(&example_graph());
    let dir = tmp.path();
    let app = trusted_app(dir);
    let recorder = Arc::new(Recorder::default());
    let (run_id, join) = app
        .start_run(dir, RunTarget::All, recorder.clone())
        .unwrap();
    assert_eq!(finish(join).await.status, RunStatus::Succeeded);

    let join = app
        .send_message(
            dir,
            &run_id,
            "survey",
            "Just answer. FAKE_NO_STATUS",
            recorder.clone(),
        )
        .await
        .unwrap()
        .unwrap();
    let after = finish(join).await;
    assert_eq!(after.nodes["survey"].status, NodeStatus::Succeeded);
    assert_eq!(after.nodes["report"].status, NodeStatus::Succeeded);
    assert_eq!(after.status, RunStatus::Succeeded);
    let summary = run_store::read_summary(dir, &run_id, "survey")
        .unwrap()
        .unwrap();
    assert_eq!(summary.status, NodeStatus::Succeeded);

    let join = app
        .send_message(dir, &run_id, "survey", "FAKE_BLOCKED now", recorder.clone())
        .await
        .unwrap()
        .unwrap();
    let after = finish(join).await;
    assert_eq!(after.nodes["survey"].status, NodeStatus::Blocked);
    assert_eq!(after.status, RunStatus::Failed);
    assert!(after.nodes["survey"]
        .reason
        .as_deref()
        .unwrap()
        .contains("May I continue"));
}

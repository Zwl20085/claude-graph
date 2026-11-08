//! End-to-end: terminal mode through scripts/fake-claude.mjs in interactive
//! mode, inside a real pty (ConPTY on Windows). Needs `node`.

mod common;

use std::fs;
use std::sync::Arc;

use claude_graph_lib::contract::{
    Graph, NodeEventKind, NodeStatus, RunMode, RunState, RunStatus, RunTarget, SessionStatus,
};
use claude_graph_lib::run_store;
use claude_graph_lib::terminal::api as term;
use common::term::*;
use common::*;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread")]
async fn terminal_run_types_messages_in_order_and_keeps_sessions_alive() {
    use_fake_claude("");
    let g = graph(
        vec![
            node(
                "a",
                0.0,
                100.0,
                json!({ "prompt": "first task", "model": "opus" }),
            ),
            node(
                "b",
                0.0,
                200.0,
                json!({ "goal": "Write b.", "deliverables": [{ "path": "b.md" }] }),
            ),
            node("c", 0.0, 300.0, json!({ "prompt": "line one\nline two" })),
            node("z", 400.0, 0.0, json!({ "prompt": "solo" })),
        ],
        &[("a", "b"), ("b", "c")],
        settings(2, false),
    );
    let tmp = graph_dir(&g);
    let dir = tmp.path();
    let app = trusted_app(dir);
    let rec = Arc::new(Recorder::default());
    let (run_id, state) = run(&app, dir, RunTarget::All, &rec).await;

    assert_eq!(state.mode, RunMode::Terminal);
    assert_eq!(state.status, RunStatus::Succeeded, "{state:#?}");
    for (id, n) in &state.nodes {
        assert_eq!(n.status, NodeStatus::Succeeded, "{id}: {:?}", n.reason);
    }
    // z sits higher, so its component is s1; a, b, c are s2.
    let sids: Vec<(&str, &str)> = state
        .sessions
        .iter()
        .map(|s| (s.sid.as_str(), s.title.as_str()))
        .collect();
    assert_eq!(sids, [("s1", "T z"), ("s2", "T a")]);
    assert!(state
        .sessions
        .iter()
        .all(|s| s.status == SessionStatus::Done && s.pid.is_some()));
    assert_eq!(received(dir, &run_id, "s1"), ["solo"]);
    let s2 = received(dir, &run_id, "s2");
    assert_eq!(s2.len(), 3, "{s2:?}");
    assert_eq!(s2[0], "first task");
    assert!(
        s2[1].starts_with("Node \"T b\" (b): read the brief at ")
            && s2[1].ends_with("/nodes/b/deliverables/."),
        "{}",
        s2[1]
    );
    assert!(
        s2[2].starts_with("line one") && s2[2].contains("line two"),
        "pasted: {}",
        s2[2]
    );

    // Brief without the finish protocol; deliverable written; files per node.
    let brief = fs::read_to_string(dir.join("nodes/b/brief.md")).unwrap();
    assert!(
        !brief.contains("## How to finish")
            && brief.contains("### T a (id: a) — status: succeeded")
    );
    assert!(dir.join("nodes/b/deliverables/b.md").is_file());
    let result = run_store::read_node_result(dir, &run_id, "b").unwrap();
    assert_eq!(result.summary.unwrap().status, NodeStatus::Succeeded);
    let summary: Value = serde_json::from_str(
        &fs::read_to_string(dir.join("runs").join(&run_id).join("b/summary.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(summary["session"], "s2");
    // Each turn's screen text: reply.md (= result.md) and its transcript span.
    let a_dir = dir.join("runs").join(&run_id).join("a");
    let reply = fs::read_to_string(a_dir.join("reply.md")).unwrap();
    assert!(reply.starts_with("> first task\n\n"), "{reply}");
    assert!(reply.contains("you said: first task"), "{reply}");
    assert!(!reply.contains("you said: line one"), "{reply}");
    let a_result = run_store::read_node_result(dir, &run_id, "a").unwrap();
    assert_eq!(a_result.result_md.as_deref(), Some(reply.as_str()));
    let a_summary: Value =
        serde_json::from_str(&fs::read_to_string(a_dir.join("summary.json")).unwrap()).unwrap();
    let (from, to) = (
        a_summary["transcript_from"].as_u64().unwrap(),
        a_summary["transcript_to"].as_u64().unwrap(),
    );
    assert!(from < to, "{from}..{to}");
    let c_reply = fs::read_to_string(dir.join("runs").join(&run_id).join("c/reply.md")).unwrap();
    // The exact final text (Stop payload), every message line quoted.
    assert!(
        c_reply.starts_with("> line one\n> line two\n\n"),
        "{c_reply}"
    );
    assert!(c_reply.contains("you said: line one"), "{c_reply}");
    let session: Value = serde_json::from_str(
        &fs::read_to_string(
            dir.join("runs")
                .join(&run_id)
                .join("sessions/s2/session.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(session["status"], "done");
    // hooks.json lives outside the graph folder; the session folder points at it.
    let hooks = hooks_dir(dir, &run_id, "s2");
    assert!(hooks.join("hooks.json").is_file());
    assert!(!dir
        .join("runs")
        .join(&run_id)
        .join("sessions/s2/hooks.json")
        .exists());
    let log = fs::read_to_string(dir.join("GRAPH_LOG.md")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|l| l.contains("\tsucceeded\t-\t"))
            .count(),
        4,
        "{log}"
    );
    assert_eq!(run_store::read_run_state(dir, &run_id).unwrap(), state);

    // Windows were opened by the backend; pty:data went to the window labels.
    let labels: Vec<String> = rec
        .windows
        .lock()
        .unwrap()
        .iter()
        .map(|w| w.label.clone())
        .collect();
    assert!(labels.contains(&format!("term-{run_id}-s2")), "{labels:?}");
    assert!(rec.windows.lock().unwrap()[0]
        .url
        .contains(&format!("#/terminal/{run_id}/")));
    let chunks = rec.pty.lock().unwrap().clone();
    assert!(chunks
        .iter()
        .all(|(label, d)| *label == format!("term-{run_id}-{}", d.sid)));
    assert!(!rec.events_of("a", NodeEventKind::Text).is_empty());

    // pty_replay: the model switch comes before the first message.
    let replay = term::pty_replay(app.terminals(), &run_id, "s2").unwrap();
    let model = replay.text.find("Set model to opus").expect("model line");
    let first = replay
        .text
        .find("you said: first task")
        .expect("first message");
    assert!(model < first);
    let last_seq = chunks
        .iter()
        .filter(|(_, d)| d.sid == "s2")
        .map(|(_, d)| d.seq)
        .max()
        .unwrap();
    assert_eq!(replay.seq, last_seq);

    // The session stays alive: typing and a follow-up message reach it.
    let listed = term::list_sessions(app.terminals(), None, &run_id).unwrap();
    assert_eq!(listed.len(), 2);
    term::pty_write(app.terminals(), &run_id, "s1", "typed by hand\r").unwrap();
    app.send_message(dir, &run_id, "c", "a follow-up", rec.clone())
        .await
        .unwrap();
    wait_until("typed lines", || {
        received(dir, &run_id, "s1").len() == 2 && received(dir, &run_id, "s2").len() == 4
    })
    .await;
    term::pty_resize(app.terminals(), &run_id, "s1", 100, 30).unwrap();

    // Cancel after the run: processes are killed.
    let pids: Vec<u32> = listed.iter().filter_map(|s| s.pid).collect();
    assert_eq!(pids.len(), 2);
    kill_sessions(&app, &run_id).await;
    assert!(pids.iter().all(|p| !process_alive(*p)), "{pids:?}");
    let after = term::list_sessions(app.terminals(), None, &run_id).unwrap();
    assert!(after.iter().all(|s| s.pid.is_none()));
    assert_eq!(
        term::pty_write(app.terminals(), &run_id, "s1", "x")
            .unwrap_err()
            .code,
        "not_found"
    );
}

fn chain(stop_on_failure: bool) -> Graph {
    graph(
        vec![
            node("a", 0.0, 0.0, json!({ "prompt": "step a" })),
            node(
                "b",
                0.0,
                100.0,
                json!({ "goal": "FAKE_FAIL", "deliverables": [{ "path": "b.md" }] }),
            ),
            node("c", 0.0, 200.0, json!({ "prompt": "step c" })),
            node("x", 300.0, 0.0, json!({ "prompt": "other component" })),
        ],
        &[("a", "b"), ("b", "c")],
        settings(2, stop_on_failure),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_on_failure_and_targets() {
    use_fake_claude("");
    // stop_on_failure: c is never sent.
    let tmp = graph_dir(&chain(true));
    let dir = tmp.path();
    let app = trusted_app(dir);
    let rec = Arc::new(Recorder::default());
    let (run1, state) = run(&app, dir, RunTarget::All, &rec).await;
    assert_eq!(state.status, RunStatus::Failed);
    let at = |s: &RunState, id: &str| (s.nodes[id].status, s.nodes[id].reason.clone());
    assert_eq!(at(&state, "a").0, NodeStatus::Succeeded);
    assert_eq!(
        at(&state, "b"),
        (
            NodeStatus::Failed,
            Some("deliverables missing: b.md".into())
        )
    );
    assert_eq!(
        at(&state, "c"),
        (NodeStatus::Skipped, Some("stopped after b".into()))
    );
    assert_eq!(at(&state, "x").0, NodeStatus::Succeeded);
    assert_eq!(received(dir, &run1, "s1").len(), 2);
    let old_pids: Vec<u32> = state.sessions.iter().filter_map(|s| s.pid).collect();
    assert_eq!(old_pids.len(), 2);

    // A new run of the graph closes the sessions of the earlier one first.
    let (run2, state) = run(&app, dir, RunTarget::From("c".into()), &rec).await;
    let closed = format!("closed sessions of run {run1}");
    let said = rec
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e.run_id == run2 && e.kind == NodeEventKind::Text && e.text == closed);
    assert!(said, "no '{closed}' event");
    wait_until("old sessions to die", || {
        old_pids.iter().all(|p| !process_alive(*p))
    })
    .await;
    let old = term::list_sessions(app.terminals(), None, &run1).unwrap();
    assert!(old.iter().all(|s| s.pid.is_none()), "{old:?}");

    // FromNode: only c's component, from c on; a and b are skipped before the start.
    assert_eq!(state.nodes.len(), 3, "x is in another component");
    assert_eq!(
        at(&state, "a"),
        (NodeStatus::Skipped, Some("before start node".into()))
    );
    assert_eq!(
        at(&state, "b"),
        (NodeStatus::Skipped, Some("before start node".into()))
    );
    assert_eq!(at(&state, "c").0, NodeStatus::Succeeded);
    assert_eq!(state.status, RunStatus::Succeeded);
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].sid, "s1");
    assert_eq!(received(dir, &run2, "s1"), ["step c"]);
    kill_sessions(&app, &run2).await;

    // FailedOf the first run: starts at b (its first failed node).
    let (run3, state) = run(&app, dir, RunTarget::FailedOf(run1.clone()), &rec).await;
    assert_eq!(at(&state, "a").1.as_deref(), Some("before start node"));
    assert_eq!(at(&state, "b").0, NodeStatus::Failed);
    kill_sessions(&app, &run3).await;

    // Without stop_on_failure the session carries on after the failure.
    let tmp = graph_dir(&chain(false));
    let dir = tmp.path();
    let app = trusted_app(dir);
    let (run4, state) = run(&app, dir, RunTarget::All, &rec).await;
    assert_eq!(at(&state, "b").0, NodeStatus::Failed);
    assert_eq!(at(&state, "c").0, NodeStatus::Succeeded);
    assert_eq!(state.status, RunStatus::Failed);
    kill_sessions(&app, &run4).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn max_parallel_caps_running_sessions() {
    use_fake_claude("");
    let nodes = (0..3)
        .map(|i| {
            node(
                &format!("n{i}"),
                300.0 * f64::from(i),
                0.0,
                json!({ "prompt": "FAKE_SLEEP_MS=700 wait" }),
            )
        })
        .collect();
    let tmp = graph_dir(&graph(nodes, &[], settings(1, false)));
    let dir = tmp.path();
    let app = trusted_app(dir);
    let rec = Arc::new(Recorder::default());
    let (run_id, state) = run(&app, dir, RunTarget::All, &rec).await;
    assert_eq!(state.status, RunStatus::Succeeded);
    let mut spans: Vec<_> = state
        .nodes
        .values()
        .map(|n| (n.started_at.unwrap(), n.finished_at.unwrap()))
        .collect();
    spans.sort();
    for w in spans.windows(2) {
        assert!(
            w[1].0 >= w[0].1,
            "sessions overlapped with max_parallel 1: {spans:?}"
        );
    }
    // Queued sessions were not even spawned while another one was working.
    let first_states: Vec<_> = rec
        .states
        .lock()
        .unwrap()
        .iter()
        .map(|s| s.sessions.iter().filter(|x| x.pid.is_some()).count())
        .collect();
    assert!(first_states.contains(&1));
    kill_sessions(&app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_kills_the_pty_child() {
    use_fake_claude("");
    let g = graph(
        vec![
            node(
                "slow",
                0.0,
                0.0,
                json!({ "prompt": "FAKE_SLEEP_MS=60000 take your time" }),
            ),
            node("after", 0.0, 100.0, json!({ "prompt": "never sent" })),
        ],
        &[("slow", "after")],
        settings(1, false),
    );
    let tmp = graph_dir(&g);
    let dir = tmp.path();
    let app = trusted_app(dir);
    let rec = Arc::new(Recorder::default());
    let (run_id, join) = app.start_by_mode(dir, RunTarget::All, rec.clone()).unwrap();
    wait_until("the slow node to be sent", || {
        received(dir, &run_id, "s1").len() == 1
    })
    .await;
    let pid = term::list_sessions(app.terminals(), None, &run_id).unwrap()[0]
        .pid
        .unwrap();
    assert!(process_alive(pid));
    app.cancel_run(&run_id).unwrap();
    let state = finish(join).await;
    assert_eq!(state.status, RunStatus::Cancelled);
    assert_eq!(state.nodes["slow"].status, NodeStatus::Cancelled);
    assert_eq!(state.nodes["after"].status, NodeStatus::Cancelled);
    assert_eq!(state.sessions[0].status, SessionStatus::Cancelled);
    wait_until("session task end", || app.terminals().live_count() == 0).await;
    assert!(!process_alive(pid), "pty child {pid} still alive");
}

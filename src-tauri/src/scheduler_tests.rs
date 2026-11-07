use serde_json::{json, Value};

use super::*;
use crate::contract::NodeRunState;

/// nodes: (id, tags); edges: (from, to).
fn graph(nodes: &[(&str, &[&str])], edges: &[(&str, &str)], settings: Value) -> Graph {
    serde_json::from_value(json!({
        "version": 1,
        "name": "t",
        "settings": settings,
        "nodes": nodes.iter().map(|(id, tags)| json!({ "id": id, "resources": tags }))
            .collect::<Vec<_>>(),
        "edges": edges.iter().enumerate()
            .map(|(i, (f, t))| json!({ "id": format!("e{i}"), "from": f, "to": t }))
            .collect::<Vec<_>>(),
    }))
    .unwrap()
}

fn run(run_id: &str, statuses: &[(&str, NodeStatus)]) -> RunState {
    RunState {
        run_id: run_id.into(),
        graph_dir: "/g".into(),
        started_at: "2026-11-07T10:00:00Z".parse().unwrap(),
        finished_at: None,
        status: crate::contract::RunStatus::Failed,
        nodes: statuses
            .iter()
            .map(|(id, s)| {
                let state = NodeRunState {
                    status: *s,
                    ..NodeRunState::pending()
                };
                (id.to_string(), state)
            })
            .collect(),
    }
}

fn ok(s: &mut Scheduler, id: &str) {
    s.on_finished(id, NodeStatus::Succeeded, None);
}

/// Drive the scheduler to completion with every node succeeding; record which
/// nodes were running together at every step.
fn simulate(s: &mut Scheduler) -> Vec<Vec<String>> {
    let mut snapshots = Vec::new();
    loop {
        s.ready();
        let running = s.running();
        if running.is_empty() {
            assert!(s.is_done());
            return snapshots;
        }
        snapshots.push(running.clone());
        // Finish the alphabetically first running node, then reschedule.
        ok(s, &running[0]);
    }
}

#[test]
fn chain_runs_in_order() {
    let g = graph(
        &[("a", &[]), ("b", &[]), ("c", &[])],
        &[("a", "b"), ("b", "c")],
        json!({}),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a"]);
    assert_eq!(s.status("b"), Some(NodeStatus::Pending));
    assert!(s.ready().is_empty(), "a still running");
    ok(&mut s, "a");
    assert_eq!(s.ready(), vec!["b"]);
    ok(&mut s, "b");
    assert_eq!(s.ready(), vec!["c"]);
    ok(&mut s, "c");
    assert!(s.is_done());
    assert!(s.run_succeeded());
}

#[test]
fn cpu_tagged_nodes_never_overlap_but_untagged_runs_alongside() {
    let g = graph(
        &[("a", &["cpu"]), ("b", &["cpu"]), ("c", &[])],
        &[],
        json!({ "max_parallel": 3, "resources": { "cpu": 1 } }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    let first = s.ready();
    assert_eq!(first, vec!["a", "c"], "one cpu node plus the untagged one");
    assert_eq!(s.status("b"), Some(NodeStatus::Queued));

    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    let snapshots = simulate(&mut s);
    for running in &snapshots {
        let cpu = running.iter().filter(|id| *id != "c").count();
        assert!(cpu <= 1, "cpu nodes overlapped: {running:?}");
    }
    assert!(
        snapshots
            .iter()
            .any(|r| r.len() == 2 && r.contains(&"c".to_string())),
        "c ran in parallel with a cpu node: {snapshots:?}"
    );
    assert!(s.run_succeeded());
}

#[test]
fn unknown_tag_defaults_to_capacity_one_and_capacity_two_allows_two() {
    let g = graph(
        &[
            ("a", &["femm"]),
            ("b", &["femm"]),
            ("x", &["gpu"]),
            ("y", &["gpu"]),
        ],
        &[],
        json!({ "max_parallel": 4, "resources": { "gpu": 2 } }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a", "x", "y"]);
}

#[test]
fn exclusive_runs_alone_and_blocks_others() {
    let g = graph(
        &[("big", &["exclusive"]), ("p", &[]), ("q", &[]), ("r", &[])],
        &[("big", "r")],
        json!({ "max_parallel": 4 }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    // big has the longest downstream path, so it goes first, alone.
    assert_eq!(s.ready(), vec!["big"]);
    assert!(s.ready().is_empty(), "nothing starts while exclusive runs");
    ok(&mut s, "big");
    assert_eq!(s.ready(), vec!["p", "q", "r"]);

    // An exclusive node that becomes ready while others run waits for them
    // and holds back lower-priority nodes.
    let g = graph(
        &[("a", &[]), ("ex", &["exclusive"]), ("late", &[])],
        &[],
        json!({ "max_parallel": 4 }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    let snapshots = simulate(&mut s);
    for running in &snapshots {
        if running.contains(&"ex".to_string()) {
            assert_eq!(running.len(), 1, "exclusive overlapped: {running:?}");
        }
    }
}

#[test]
fn max_parallel_and_priority_tie_break() {
    // d1 -> d2 -> d3 is the longest chain, so d1 starts before the leaves.
    let g = graph(
        &[
            ("a", &[]),
            ("b", &[]),
            ("d1", &[]),
            ("d2", &[]),
            ("d3", &[]),
        ],
        &[("d1", "d2"), ("d2", "d3")],
        json!({ "max_parallel": 2 }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["d1", "a"]);
    assert_eq!(s.status("b"), Some(NodeStatus::Queued));
}

#[test]
fn failure_skips_transitive_dependents_but_not_independent_branch() {
    let g = graph(
        &[("a", &[]), ("b", &[]), ("c", &[]), ("z", &[])],
        &[("a", "b"), ("b", "c")],
        json!({ "max_parallel": 1 }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a"]);
    s.on_finished("a", NodeStatus::Failed, Some("boom".into()));
    assert_eq!(s.status("b"), Some(NodeStatus::Skipped));
    assert_eq!(s.status("c"), Some(NodeStatus::Skipped));
    assert_eq!(s.reason("c"), Some("upstream a failed"));
    assert_eq!(s.ready(), vec!["z"]);
    ok(&mut s, "z");
    assert!(s.is_done());
    assert!(!s.run_succeeded());
}

#[test]
fn stop_on_failure_starts_nothing_new() {
    let g = graph(
        &[("a", &[]), ("b", &[]), ("c", &[])],
        &[],
        json!({ "max_parallel": 2, "stop_on_failure": true }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a", "b"]);
    s.on_finished("a", NodeStatus::Failed, None);
    assert!(s.ready().is_empty());
    assert!(!s.is_done(), "b still running");
    ok(&mut s, "b");
    assert!(s.is_done());
    s.finalize();
    assert_eq!(s.status("c"), Some(NodeStatus::Skipped));
}

#[test]
fn cancel_all_and_cancel_node() {
    let g = graph(
        &[("a", &[]), ("b", &[]), ("c", &[])],
        &[("a", "c")],
        json!({ "max_parallel": 1 }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a"]);
    assert_eq!(s.cancel_node("b"), CancelEffect::Cancelled);
    assert_eq!(s.cancel_node("a"), CancelEffect::MustKill);
    assert_eq!(s.cancel_node("zz"), CancelEffect::Nothing);
    s.on_finished("a", NodeStatus::Cancelled, None);
    assert_eq!(s.status("c"), Some(NodeStatus::Skipped));

    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a"]);
    assert_eq!(s.cancel_all(), vec!["a"]);
    assert_eq!(s.status("b"), Some(NodeStatus::Cancelled));
    assert!(s.ready().is_empty());
    s.on_finished("a", NodeStatus::Cancelled, None);
    assert!(s.is_done());
    assert!(s.is_cancelled());
}

#[test]
fn from_target_uses_prior_success_of_upstream() {
    let g = graph(
        &[("a", &[]), ("b", &[]), ("c", &[]), ("other", &[])],
        &[("a", "b"), ("b", "c")],
        json!({}),
    );
    let prior = [
        run("20261107T090000Z", &[("a", NodeStatus::Succeeded)]),
        run("20261107T100000Z", &[("a", NodeStatus::Succeeded)]),
        run("20261107T110000Z", &[("a", NodeStatus::Failed)]),
    ];
    let mut s = Scheduler::new(&g, &RunTarget::From("b".into()), &prior).unwrap();
    assert_eq!(
        s.target().iter().cloned().collect::<Vec<_>>(),
        vec!["b", "c"]
    );
    assert_eq!(s.prior_run_of("a"), Some("20261107T100000Z"));
    assert_eq!(s.ready(), vec!["b"]);

    // Without any earlier success of `a`, `b` and `c` are skipped up front.
    let s = Scheduler::new(&g, &RunTarget::From("b".into()), &[]).unwrap();
    assert_eq!(s.status("b"), Some(NodeStatus::Skipped));
    assert_eq!(s.status("c"), Some(NodeStatus::Skipped));
    assert!(s.is_done());

    let err = Scheduler::new(&g, &RunTarget::From("nope".into()), &[]).unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[test]
fn failed_of_target_reruns_failed_nodes_and_dependents() {
    let g = graph(
        &[("a", &[]), ("b", &[]), ("c", &[]), ("z", &[])],
        &[("a", "b"), ("b", "c")],
        json!({}),
    );
    let prior = [run(
        "R1",
        &[
            ("a", NodeStatus::Succeeded),
            ("b", NodeStatus::Failed),
            ("c", NodeStatus::Skipped),
            ("z", NodeStatus::Succeeded),
        ],
    )];
    let s = Scheduler::new(&g, &RunTarget::FailedOf("R1".into()), &prior).unwrap();
    assert_eq!(
        s.target().iter().cloned().collect::<Vec<_>>(),
        vec!["b", "c"]
    );
    assert_eq!(s.prior_run_of("a"), Some("R1"));
    let err = Scheduler::new(&g, &RunTarget::FailedOf("R9".into()), &prior).unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[test]
fn invalid_graph_is_rejected() {
    let g = graph(
        &[("a", &[]), ("b", &[])],
        &[("a", "b"), ("b", "a")],
        json!({}),
    );
    let err = Scheduler::new(&g, &RunTarget::All, &[]).unwrap_err();
    assert_eq!(err.code, "invalid_graph");
}

#[test]
fn node_with_queued_message_is_not_finished_until_queue_drains() {
    let g = graph(&[("a", &[]), ("b", &[])], &[("a", "b")], json!({}));
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a"]);
    assert!(s.hold("a"), "message queued while a runs");
    assert!(s.hold("a"), "second message");
    // The first process ends: a is still running, b must not start.
    s.on_finished("a", NodeStatus::Succeeded, None);
    assert_eq!(s.status("a"), Some(NodeStatus::Running));
    assert!(s.ready().is_empty());
    assert!(!s.is_done());
    // First follow-up turn ends.
    s.release("a");
    s.on_finished("a", NodeStatus::Succeeded, None);
    assert_eq!(s.status("a"), Some(NodeStatus::Running));
    assert!(s.ready().is_empty());
    // Second follow-up turn ends: queue drained.
    s.release("a");
    s.on_finished("a", NodeStatus::Succeeded, None);
    assert_eq!(s.status("a"), Some(NodeStatus::Succeeded));
    assert_eq!(s.ready(), vec!["b"]);
}

#[test]
fn finished_node_can_be_reopened_and_holds_its_pending_dependents() {
    let g = graph(
        &[("a", &[]), ("b", &[]), ("c", &[])],
        &[("a", "b")],
        json!({ "max_parallel": 1 }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a"]);
    ok(&mut s, "a");
    assert!(!s.hold("a"), "hold only applies to a running node");
    assert!(s.reopen("a"));
    assert_eq!(s.status("a"), Some(NodeStatus::Running));
    assert!(s.ready().is_empty(), "max_parallel 1 and a runs again");
    s.release("a");
    s.on_finished("a", NodeStatus::Succeeded, None);
    assert_eq!(s.ready(), vec!["b"]);
    assert!(!s.reopen("c"), "c never ran");
}

#[test]
fn cancel_drops_holds_and_resume_starts_nothing() {
    let g = graph(&[("a", &[]), ("b", &[])], &[("a", "b")], json!({}));
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    s.ready();
    s.hold("a");
    s.on_finished("a", NodeStatus::Cancelled, None);
    assert_eq!(s.status("a"), Some(NodeStatus::Cancelled));
    assert_eq!(s.status("b"), Some(NodeStatus::Skipped));

    let prior = run(
        "R1",
        &[("a", NodeStatus::Succeeded), ("b", NodeStatus::Pending)],
    );
    let mut s = Scheduler::resume(&g, &prior).unwrap();
    assert!(s.ready().is_empty());
    assert!(s.reopen("a"));
    assert!(!s.is_done());
    s.release("a");
    s.on_finished("a", NodeStatus::Failed, Some("BLOCKED".into()));
    assert!(s.is_done());
    // b was still pending in run.json: resume marks it interrupted (P3).
    assert_eq!(s.status("b"), Some(NodeStatus::Cancelled));
    assert_eq!(s.reason("b"), Some("interrupted"));
}

#[test]
fn resume_maps_unfinished_slots_to_interrupted() {
    let g = graph(
        &[("a", &[]), ("b", &[]), ("c", &[]), ("d", &[])],
        &[],
        json!({}),
    );
    let r = run(
        "R1",
        &[
            ("a", NodeStatus::Succeeded),
            ("b", NodeStatus::Running),
            ("c", NodeStatus::Queued),
            ("d", NodeStatus::Pending),
        ],
    );
    let mut s = Scheduler::resume(&g, &r).unwrap();
    assert_eq!(s.status("a"), Some(NodeStatus::Succeeded));
    for id in ["b", "c", "d"] {
        assert_eq!(s.status(id), Some(NodeStatus::Cancelled), "{id}");
        assert_eq!(s.reason(id), Some("interrupted"));
    }
    assert!(s.running().is_empty());
    assert!(s.ready().is_empty());
    assert!(s.is_done());
}

#[test]
fn blocked_is_scheduled_like_failed() {
    let g = graph(
        &[("a", &[]), ("b", &[]), ("c", &[]), ("z", &[])],
        &[("a", "b"), ("b", "c")],
        json!({ "max_parallel": 1 }),
    );
    let mut s = Scheduler::new(&g, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a"]);
    s.on_finished("a", NodeStatus::Blocked, Some("May I use the web?".into()));
    assert_eq!(s.status("a"), Some(NodeStatus::Blocked));
    assert_eq!(s.reason("a"), Some("May I use the web?"));
    for id in ["b", "c"] {
        assert_eq!(s.status(id), Some(NodeStatus::Skipped));
        assert_eq!(s.reason(id), Some("upstream a blocked"));
    }
    assert_eq!(s.ready(), vec!["z"]);
    ok(&mut s, "z");
    assert!(s.is_done());
    assert!(!s.run_succeeded(), "a blocked node fails the run");

    // stop_on_failure applies to blocked nodes too.
    let g2 = graph(
        &[("a", &[]), ("b", &[]), ("c", &[])],
        &[],
        json!({ "max_parallel": 2, "stop_on_failure": true }),
    );
    let mut s = Scheduler::new(&g2, &RunTarget::All, &[]).unwrap();
    assert_eq!(s.ready(), vec!["a", "b"]);
    s.on_finished("a", NodeStatus::Blocked, None);
    assert!(s.ready().is_empty());
    ok(&mut s, "b");
    assert!(s.is_done());
    s.finalize();
    assert_eq!(s.status("c"), Some(NodeStatus::Skipped));

    // FailedOf targets blocked nodes and their dependents.
    let prior = [run(
        "R1",
        &[
            ("a", NodeStatus::Blocked),
            ("b", NodeStatus::Skipped),
            ("c", NodeStatus::Skipped),
            ("z", NodeStatus::Succeeded),
        ],
    )];
    let s = Scheduler::new(&g, &RunTarget::FailedOf("R1".into()), &prior).unwrap();
    assert_eq!(
        s.target().iter().cloned().collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
}

#[test]
fn resume_keeps_blocked_and_a_follow_up_can_promote_it() {
    let g = graph(&[("a", &[]), ("b", &[])], &[("a", "b")], json!({}));
    let prior = run(
        "R1",
        &[("a", NodeStatus::Blocked), ("b", NodeStatus::Skipped)],
    );
    let mut s = Scheduler::resume(&g, &prior).unwrap();
    assert_eq!(s.status("a"), Some(NodeStatus::Blocked));
    assert!(s.is_done());
    assert!(s.reopen("a"), "a blocked node takes follow-ups");
    assert_eq!(s.status("a"), Some(NodeStatus::Running));
    s.release("a");
    ok(&mut s, "a");
    assert_eq!(s.status("a"), Some(NodeStatus::Succeeded));
    assert_eq!(s.status("b"), Some(NodeStatus::Skipped), "skips stay");
}

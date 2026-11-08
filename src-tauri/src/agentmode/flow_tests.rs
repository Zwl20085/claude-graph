//! The engine end to end on disk (open → next → report …), without MCP or CLI.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::contract::{Graph, NodeStatus, RunStatus};
use crate::run_store::{node_run_dir, read_run_state, LOG_FILE, RESULT_FILE, SUMMARY_FILE};
use crate::store::{deliverables_dir, save_graph};
use crate::terminal::questions;

use super::report::{Outcome, ReportInput};
use super::{next, open, report, run, Caller, Channel};

/// plan -> build (gpu) -> check; lone (gpu) on its own. One gpu.
pub fn fixture(dir: &Path) -> PathBuf {
    let g: Graph = serde_json::from_value(json!({
        "version": 1, "name": "fixture",
        "settings": { "max_parallel": 2, "resources": { "gpu": 1 } },
        "nodes": [
            { "id": "plan", "title": "Plan", "goal": "Write the plan",
              "deliverables": [{ "path": "plan.md" }] },
            { "id": "build", "title": "Build", "goal": "Build it", "resources": ["gpu"] },
            { "id": "check", "title": "Check", "prompt": "Check the build" },
            { "id": "lone", "title": "Lone", "prompt": "Side job", "resources": ["gpu"] }
        ],
        "edges": [
            { "id": "e1", "from": "plan", "to": "build" },
            { "id": "e2", "from": "build", "to": "check" }
        ]
    }))
    .unwrap();
    save_graph(dir, &g).unwrap();
    run::graph_dir(&dir.to_string_lossy(), false).unwrap()
}

fn caller() -> Caller {
    Caller {
        client: "Claude Code".into(),
        channel: Channel::Mcp,
    }
}

fn input(node: &str, outcome: Outcome, summary: &str) -> ReportInput {
    ReportInput {
        node_id: node.into(),
        outcome,
        summary: summary.into(),
        question: None,
        deliverables_written: Vec::new(),
    }
}

fn ids(n: &crate::contract_agent::AgentNext) -> Vec<&str> {
    n.nodes.iter().map(|b| b.node_id.as_str()).collect()
}

#[test]
fn full_loop_with_missing_deliverable_retry_and_upstream_reply() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path());
    let c = caller();
    let o = open::open(&dir, &c, false).unwrap();
    assert!(!o.attached);
    let pos = |id: &str| o.nodes.iter().position(|n| n.id == id).unwrap();
    assert!(pos("plan") < pos("build") && pos("build") < pos("check"));
    let rid = o.run_id.clone();
    // Re-open attaches to the same run.
    let again = open::open(&dir, &c, false).unwrap();
    assert!(again.attached && again.run_id == rid);

    let n = next::next(&dir, &rid, &c, 4, None).unwrap();
    assert_eq!(ids(&n), ["plan", "lone"]);
    assert!(n.nodes[0].brief.contains("## Report back"));
    assert!(n.nodes[0].brief.contains(&format!("run_id \"{rid}\"")));
    // Never handed out twice.
    let n2 = next::next(&dir, &rid, &c, 4, None).unwrap();
    assert!(n2.nodes.is_empty() && !n2.done);
    assert_eq!(n2.waiting_on, ["lone", "plan"]);

    // done without the deliverable: failed, dependents skipped.
    let r = report::report(&dir, &rid, &c, &input("plan", Outcome::Done, "wrote it")).unwrap();
    assert_eq!(r.status, NodeStatus::Failed);
    assert_eq!(r.reason.as_deref(), Some("deliverables missing: plan.md"));
    let st = read_run_state(&dir, &rid).unwrap();
    assert_eq!(st.nodes["build"].status, NodeStatus::Skipped);
    // A second report of a settled node is refused.
    assert!(report::report(&dir, &rid, &c, &input("plan", Outcome::Done, "x")).is_err());

    // Retry: plan and its skipped dependents come back.
    let n = next::next(&dir, &rid, &c, 1, Some("plan")).unwrap();
    assert_eq!(ids(&n), ["plan"]);
    assert!(n.nodes[0].brief.contains("## Previous attempt in this run"));
    std::fs::write(deliverables_dir(&dir, "plan").join("plan.md"), "the plan").unwrap();
    let r = report::report(&dir, &rid, &c, &input("plan", Outcome::Done, "PLAN-REPLY")).unwrap();
    assert_eq!(r.status, NodeStatus::Succeeded);
    assert_eq!(r.ready, ["build"]);
    // build needs the gpu, which lone holds.
    let n = next::next(&dir, &rid, &c, 1, None).unwrap();
    assert!(n.nodes.is_empty(), "gpu is taken by lone");
    report::report(&dir, &rid, &c, &input("lone", Outcome::Done, "ok")).unwrap();
    let n = next::next(&dir, &rid, &c, 1, None).unwrap();
    assert_eq!(ids(&n), ["build"]);
    assert!(
        n.nodes[0].brief.contains("PLAN-REPLY"),
        "upstream reply inlined"
    );

    // Files of a reported node.
    let folder = node_run_dir(&dir, &rid, "plan");
    let result = std::fs::read_to_string(folder.join(RESULT_FILE)).unwrap();
    assert!(result.contains("PLAN-REPLY"));
    let summary_text = std::fs::read_to_string(folder.join(SUMMARY_FILE)).unwrap();
    let summary: crate::contract::NodeSummary = serde_json::from_str(&summary_text).unwrap();
    assert_eq!(summary.status, NodeStatus::Succeeded);
    let log = std::fs::read_to_string(dir.join(LOG_FILE)).unwrap();
    assert!(log.contains(&format!("{rid}\tplan\tsucceeded")));

    report::report(&dir, &rid, &c, &input("build", Outcome::Done, "built")).unwrap();
    let n = next::next(&dir, &rid, &c, 1, None).unwrap();
    assert_eq!(ids(&n), ["check"]);
    let r = report::report(&dir, &rid, &c, &input("check", Outcome::Done, "fine")).unwrap();
    assert!(r.done);
    let n = next::next(&dir, &rid, &c, 1, None).unwrap();
    assert!(n.done);
    assert_eq!(n.summary.unwrap().run_status, RunStatus::Succeeded);
    assert!(run::active(&dir).is_none());
}

#[test]
fn blocked_pauses_dependents_and_records_the_question() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path());
    let c = caller();
    let rid = open::open(&dir, &c, false).unwrap().run_id;
    next::next(&dir, &rid, &c, 1, None).unwrap();
    let blocked = ReportInput {
        question: Some("Which database?".into()),
        ..input("plan", Outcome::Blocked, "need a choice")
    };
    let r = report::report(&dir, &rid, &c, &blocked).unwrap();
    assert_eq!(r.status, NodeStatus::Waiting);
    let st = read_run_state(&dir, &rid).unwrap();
    assert_eq!(
        st.nodes["build"].status,
        NodeStatus::Pending,
        "paused, not skipped"
    );
    let qs = questions::read(&node_run_dir(&dir, &rid, "plan"));
    assert_eq!(qs.len(), 1);
    assert_eq!(qs[0].question, "Which database?");
    let n = next::next(&dir, &rid, &c, 4, None).unwrap();
    assert_eq!(ids(&n), ["lone"]);
    assert_eq!(n.blocked[0].question, "Which database?");
    // A blocked node may be reported done directly once answered in chat.
    std::fs::write(deliverables_dir(&dir, "plan").join("plan.md"), "p").unwrap();
    let r = report::report(
        &dir,
        &rid,
        &c,
        &input("plan", Outcome::Done, "chose sqlite"),
    )
    .unwrap();
    assert_eq!(r.status, NodeStatus::Succeeded);
}

#[test]
fn stop_new_run_and_app_run_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path());
    let c = caller();
    let rid = open::open(&dir, &c, false).unwrap().run_id;
    next::next(&dir, &rid, &c, 1, None).unwrap();
    let ended = open::abandon(&dir, &rid).unwrap();
    assert_eq!(ended.state.status, RunStatus::Cancelled);
    assert_eq!(
        ended.state.nodes["plan"].reason.as_deref(),
        Some(open::STOPPED)
    );
    let err = next::next(&dir, &rid, &c, 1, None).unwrap_err();
    assert_eq!(err.code, "run_cancelled");
    assert!(report::report(&dir, &rid, &c, &input("plan", Outcome::Done, "")).is_err());
    let second = open::open(&dir, &c, false).unwrap();
    assert!(!second.attached && second.run_id != rid);
    let third = open::open(&dir, &c, true).unwrap();
    assert_ne!(third.run_id, second.run_id);
    let old = run::read_agent(&dir, &second.run_id).unwrap();
    assert_eq!(old.state.status, RunStatus::Cancelled);
    // A running app run blocks graph_open.
    let mut app = read_run_state(&dir, &rid).unwrap();
    app.run_id = "20990101T000000Z".into();
    app.status = RunStatus::Running;
    std::fs::create_dir_all(crate::run_store::run_dir(&dir, &app.run_id)).unwrap();
    crate::run_store::write_run_state(&dir, &app).unwrap();
    assert_eq!(open::open(&dir, &c, false).unwrap_err().code, "run_active");
}

#[test]
fn notes_and_status() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path());
    let c = caller();
    assert_eq!(open::status(&dir, None).unwrap_err().code, "not_found");
    let rid = open::open(&dir, &c, false).unwrap().run_id;
    open::note(&dir, Some(&rid), None, "starting").unwrap();
    open::note(&dir, Some(&rid), Some("plan"), "on it").unwrap();
    assert!(open::note(&dir, Some(&rid), Some("ghost"), "x").is_err());
    assert!(open::note(&dir, Some(&rid), None, "  ").is_err());
    let (log, _) = run::read_log(&dir, &rid, 0);
    assert!(log
        .iter()
        .any(|e| e.text == "on it" && e.node_id.as_deref() == Some("plan")));
    let s = open::status(&dir, None).unwrap();
    assert_eq!(s.rows.len(), 4);
    assert_eq!(s.client, "Claude Code");
    assert!(super::text::status(&s).contains("plan"));
}

use super::*;
use crate::agentmode::flow_tests::fixture;
use crate::agentmode::report::{Outcome, ReportInput};
use crate::agentmode::{next, open, report, Caller, Channel};

#[test]
fn poller_sees_new_runs_reports_and_notes() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path());
    let mut p = Poller::new(dir.clone());
    assert!(p.poll().is_none(), "no agent run yet");
    let c = Caller {
        client: "Codex".into(),
        channel: Channel::Cli,
    };
    let rid = open::open(&dir, &c, false).unwrap().run_id;
    let u = p.poll().unwrap();
    assert_eq!(
        (u.info.run_id.as_str(), u.info.active),
        (rid.as_str(), true)
    );
    assert_eq!(u.info.client, "Codex");
    assert!(u.state.is_some());
    assert!(p.poll().is_none(), "nothing changed");

    next::next(&dir, &rid, &c, 1, None).unwrap();
    let plan = crate::store::deliverables_dir(&dir, "plan").join("plan.md");
    std::fs::write(plan, "x").unwrap();
    let input = ReportInput {
        node_id: "plan".into(),
        outcome: Outcome::Done,
        summary: "done".into(),
        question: None,
        deliverables_written: vec!["plan.md".into()],
    };
    report::report(&dir, &rid, &c, &input).unwrap();
    let u = p.poll().unwrap();
    let state = u.state.unwrap();
    assert_eq!(
        state.nodes["plan"].status,
        crate::contract::NodeStatus::Succeeded
    );
    assert!(u
        .events
        .iter()
        .any(|e| e.node_id == "plan" && e.text.starts_with("reported succeeded")));

    open::note(&dir, Some(&rid), None, "halfway").unwrap();
    let u = p.poll().unwrap();
    assert!(u.state.is_none());
    assert_eq!(u.info.last_note.as_deref(), Some("halfway"));
    assert_eq!(p.current().unwrap().run_id, rid);
}

#[test]
fn cancel_agent_run_only_for_the_watched_folder() {
    struct Null;
    impl Sink for Null {
        fn update(&self, _: &Update) {}
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path());
    let c = Caller {
        client: "dsh".into(),
        channel: Channel::Mcp,
    };
    let rid = open::open(&dir, &c, false).unwrap().run_id;
    let w = AgentWatch::default();
    assert!(cancel_agent_run(&w, &rid).is_none(), "nothing watched");
    let info = w
        .watch(Some(dir.clone()), std::sync::Arc::new(Null))
        .unwrap();
    assert!(info.active);
    assert!(cancel_agent_run(&w, "20000101T000000Z").is_none());
    cancel_agent_run(&w, &rid).unwrap().unwrap();
    let after = w.watch(Some(dir), std::sync::Arc::new(Null)).unwrap();
    assert!(!after.active);
}

#[test]
fn a_new_poller_does_not_replay_an_existing_run() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path());
    let c = Caller {
        client: "Claude Code".into(),
        channel: Channel::Mcp,
    };
    let rid = open::open(&dir, &c, false).unwrap().run_id;
    open::note(&dir, Some(&rid), None, "old note").unwrap();
    let mut p = Poller::new(dir.clone());
    assert!(p.poll().is_none(), "existing state is the baseline");
    assert_eq!(p.current().unwrap().last_note.as_deref(), Some("old note"));
    next::next(&dir, &rid, &c, 1, None).unwrap();
    assert!(p.poll().unwrap().state.is_some());
}

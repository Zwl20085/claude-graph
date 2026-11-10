use std::sync::Arc;

use super::*;
use crate::events::NullEmitter;
use crate::inbox::env::NullSink;
use crate::run_api::AppState;

fn inbox(folder: PathBuf, max_parallel: usize) -> (Inbox, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let cfg = RunConfig {
        enabled: true,
        folder,
        max_parallel,
        timeout: Duration::from_secs(60),
        interval: Duration::from_secs(3600),
    };
    let env = InboxEnv {
        app: AppState::new(data.path().to_path_buf()),
        emitter: Arc::new(NullEmitter),
        sink: Arc::new(NullSink),
        host: "H".into(),
        clock: Arc::new(|| chrono::Local::now().naive_local()),
        config: Arc::new(move || cfg.clone()),
        suggest: Arc::new(|| None),
        poll: Duration::from_millis(50),
        retry_delay: Duration::from_millis(50),
    };
    (Inbox::new(env), data)
}

fn pending(id: &str, folder: &Path) -> Pending {
    Pending {
        id: id.into(),
        path: folder.join(format!("{id}.md")),
        folder: folder.to_path_buf(),
    }
}

#[test]
fn taking_a_tip_reserves_its_slot_with_its_run_id_in_one_step() {
    let folder = PathBuf::from("C:/tips");
    let (ib, _data) = inbox(folder.clone(), 1);
    {
        let mut s = ib.lock();
        s.queue.push_back(pending("a", &folder));
        s.queue.push_back(pending("b", &folder));
    }
    let cfg = ib.config();
    let (p, run, _) = ib.take_next(&cfg).expect("a slot is free");
    assert_eq!(p.id, "a");
    // The placeholder is in place before any claim: it counts as working
    // and its run id as live, so a second pump finds no slot.
    assert!(ib.lock().live_runs().contains(&run));
    assert_eq!(ib.lock().working(), 1);
    assert!(ib.take_next(&cfg).is_none());
    assert_eq!(ib.lock().queue.len(), 1);
}

#[test]
fn concurrent_pumps_never_exceed_max_parallel() {
    let folder = PathBuf::from("C:/tips");
    let (ib, _data) = inbox(folder.clone(), 2);
    for i in 0..20 {
        ib.lock()
            .queue
            .push_back(pending(&format!("t{i}"), &folder));
    }
    let cfg = ib.config();
    let taken: usize = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..8)
            .map(|_| sc.spawn(|| ib.take_next(&cfg).is_some() as usize))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).sum()
    });
    assert_eq!(taken, 2);
    assert_eq!(ib.lock().working(), 2);
}

#[test]
fn tips_queued_under_another_folder_are_dropped() {
    let folder = PathBuf::from("C:/tips");
    let (ib, _data) = inbox(folder.clone(), 2);
    {
        let mut s = ib.lock();
        s.queue.push_back(pending("old", Path::new("C:/old-tips")));
        s.queue.push_back(pending("new", &folder));
    }
    let (p, _, _) = ib.take_next(&ib.config()).unwrap();
    assert_eq!(p.id, "new");
    assert!(ib.lock().queue.is_empty());
    assert!(!ib.lock().active.contains_key("old"));
    assert!(ib
        .status()
        .log
        .iter()
        .any(|l| l.text.contains("tips folder setting changed")));
}

#[test]
fn recorded_claims_survive_a_restart() {
    let folder = PathBuf::from("C:/tips");
    let (ib, data) = inbox(folder.clone(), 1);
    ib.record_claim("20261109-073100-ab12");
    let again = Inbox::new(InboxEnv::for_app(
        AppState::new(data.path().to_path_buf()),
        Arc::new(NullEmitter),
        Arc::new(NullSink),
    ));
    assert!(again.lock().claimed_set().contains("20261109-073100-ab12"));
}

/// Poll `cond` every 10 ms until it holds (generous deadline for slow CI).
async fn eventually(what: &str, cond: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(30);
    while !cond() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_folder_schedules_a_bounded_number_of_rescans() {
    let tips = tempfile::tempdir().unwrap();
    let (ib, _data) = inbox(tips.path().to_path_buf(), 1);
    let pending = || ib.0.retry_pending.load(Ordering::SeqCst);
    for _ in 0..(crate::inbox::retry::RESCAN_MAX + 3) {
        // Every round starts with no re-scan pending, whatever the speed of
        // the machine: the first call is never swallowed by the previous one.
        eventually("the pending re-scan to fire", || !pending()).await;
        ib.schedule_rescan();
        // Only one pending at a time.
        ib.schedule_rescan();
    }
    eventually("the last re-scan to fire", || !pending()).await;
    let scanned = || {
        ib.status()
            .log
            .iter()
            .any(|l| l.text.starts_with("scanned:"))
    };
    eventually("a re-scan to complete", scanned).await;
    let log = ib.status().log;
    let scheduled = log
        .iter()
        .filter(|l| l.text.starts_with("re-scan in"))
        .count();
    assert_eq!(scheduled as u32, crate::inbox::retry::RESCAN_MAX);
    assert!(log.iter().any(|l| l.text.contains("retries used up")));
}

fn tracked(ib: &Inbox, id: &str, phase: InboxPhase, graph_run: &str) -> Arc<AtomicBool> {
    let mut a = crate::inbox::start::placeholder(&ib.graphs_root(), id, &format!("run-{id}"));
    a.run.phase = phase;
    a.run.graph_run_id = Some(graph_run.to_string());
    let flag = a.superseded.clone();
    ib.lock().active.insert(id.to_string(), a);
    flag
}

/// Review finding 1: a scan queues the fresh run of a tip whose earlier
/// session is being superseded before that session's entry is removed; a
/// concurrent pump must not take it (it would overwrite the entry, and the
/// earlier session would never be closed).
#[test]
fn a_tip_whose_earlier_session_is_still_tracked_is_not_taken() {
    let folder = PathBuf::from("C:/tips");
    let (ib, _data) = inbox(folder.clone(), 2);
    tracked(&ib, "a", InboxPhase::Question, "old-graph-run");
    {
        let mut s = ib.lock();
        s.queue.push_back(pending("a", &folder));
        s.queue.push_back(pending("b", &folder));
    }
    let cfg = ib.config();
    let (p, _, _) = ib.take_next(&cfg).expect("b is free");
    assert_eq!(p.id, "b", "the blocked tip does not hold up the others");
    assert!(
        ib.take_next(&cfg).is_none(),
        "a waits for its earlier session"
    );
    {
        let s = ib.lock();
        assert_eq!(s.queue.len(), 1);
        assert_eq!(s.queue[0].id, "a");
        let old = &s.active["a"];
        assert_eq!(old.run.graph_run_id.as_deref(), Some("old-graph-run"));
        assert_eq!(old.run.phase, InboxPhase::Question);
    }
    // Once the earlier session is closed (its entry removed), it is taken.
    ib.lock().active.remove("a");
    let (p, _, _) = ib.take_next(&cfg).expect("a is free now");
    assert_eq!(p.id, "a");
}

/// Review finding 1, the supersede side: the entry is removed only once the
/// earlier session is closed.
#[tokio::test(flavor = "multi_thread")]
async fn superseding_removes_the_entry_only_after_the_session_is_closed() {
    let folder = PathBuf::from("C:/tips");
    let (ib, _data) = inbox(folder, 1);
    let flag = tracked(&ib, "a", InboxPhase::Question, "gone-run");
    assert!(ib.supersede("a").await);
    assert!(flag.load(Ordering::SeqCst));
    assert!(!ib.lock().active.contains_key("a"));
}

/// Review finding 2: a finished session is closed at the start of the next
/// scan; one waiting at a question, and one whose outcome is still to be
/// written, are not.
#[tokio::test(flavor = "multi_thread")]
async fn finished_sessions_are_closed_at_the_start_of_the_next_scan() {
    let tips = tempfile::tempdir().unwrap();
    let (ib, _data) = inbox(tips.path().to_path_buf(), 1);
    let done = tracked(&ib, "done", InboxPhase::Finished, "r-done");
    let asking = tracked(&ib, "asking", InboxPhase::Question, "r-asking");
    tracked(&ib, "kept", InboxPhase::Finished, "r-kept");
    let round = crate::inbox::watch::Round {
        tip_id: "kept".into(),
        path: tips.path().join("kept.md"),
        folder: tips.path().to_path_buf(),
        run: "run-kept".into(),
        graph_dir: ib.graphs_root().join("kept"),
        graph_run: Some("r-kept".into()),
        expected_body: String::new(),
        prompt: None,
    };
    ib.keep_unwritten(
        &round,
        &crate::inbox::block::Outcome::Failed { reason: "x".into() },
        "read-only",
    );
    ib.scan_now().await.unwrap();
    {
        let s = ib.lock();
        assert!(!s.active.contains_key("done"));
        assert!(done.load(Ordering::SeqCst));
        assert!(s.active.contains_key("asking"));
        assert!(!asking.load(Ordering::SeqCst));
        // Kept until its outcome is written (this scan dropped it: the tip
        // file is gone), then closed by the scan after.
        assert!(s.active.contains_key("kept"));
        assert!(s
            .log
            .iter()
            .any(|l| l.text == "closed the finished session (run r-done)"));
    }
    ib.scan_now().await.unwrap();
    assert!(!ib.lock().active.contains_key("kept"));
}

fn agent_tip_claimed_by(host: &str, run: &str) -> String {
    format!(
        "---\ntitle: T\nkind: agent\nagent:\n  v: 1\n  state: running\n  host: {host}\n  run: {run}\n---\nDo x\n"
    )
}

/// Review finding 4: when the claim is no longer this host's run, the
/// round's files are not copied over the results of whoever holds it now.
#[test]
fn results_are_not_copied_when_the_claim_is_lost() {
    use crate::inbox::block::Outcome;
    use crate::inbox::graphgen::NODE_ID;
    use crate::inbox::watch::{write_round, Round, Written};
    use crate::run_store::{node_run_dir, RESULT_FILE};

    let tips = tempfile::tempdir().unwrap();
    let (ib, data) = inbox(tips.path().to_path_buf(), 1);
    let tip = tips.path().join("t.md");
    let results = tips.path().join("agent").join("t");
    std::fs::create_dir_all(&results).unwrap();
    std::fs::write(results.join("result.md"), "theirs").unwrap();
    let graph_dir = data.path().join("g");
    let src = node_run_dir(&graph_dir, "g1", NODE_ID).join(RESULT_FILE);
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    std::fs::write(&src, "mine").unwrap();
    let round = Round {
        tip_id: "t".into(),
        path: tip.clone(),
        folder: tips.path().to_path_buf(),
        run: "r-mine".into(),
        graph_dir,
        graph_run: Some("g1".into()),
        expected_body: "Do x\n".into(),
        prompt: Some("the prompt".into()),
    };
    let done = Outcome::Done {
        reply: "mine".into(),
    };

    // Another machine took the tip over: nothing is copied or written.
    let theirs = agent_tip_claimed_by("OTHER", "r-other");
    std::fs::write(&tip, &theirs).unwrap();
    assert_eq!(write_round(&ib, &round, &done), Written::Dropped);
    assert_eq!(
        std::fs::read_to_string(results.join("result.md")).unwrap(),
        "theirs"
    );
    assert_eq!(std::fs::read_to_string(&tip).unwrap(), theirs);
    assert!(ib
        .status()
        .log
        .iter()
        .any(|l| l.text.starts_with("results not copied: claim lost")));

    // Another run of this machine: the same.
    std::fs::write(&tip, agent_tip_claimed_by("H", "r-later")).unwrap();
    assert_eq!(write_round(&ib, &round, &done), Written::Dropped);
    assert_eq!(
        std::fs::read_to_string(results.join("result.md")).unwrap(),
        "theirs"
    );

    // No longer an agent tip: the same.
    let task = "---\ntitle: T\nkind: task\n---\nDo x\n";
    std::fs::write(&tip, task).unwrap();
    assert_eq!(write_round(&ib, &round, &done), Written::Dropped);
    assert_eq!(std::fs::read_to_string(&tip).unwrap(), task);
    assert_eq!(
        std::fs::read_to_string(results.join("result.md")).unwrap(),
        "theirs"
    );

    assert!(!results.join("prompt.md").exists(), "no prompt either");

    // Still ours: copied and written.
    std::fs::write(&tip, agent_tip_claimed_by("H", "r-mine")).unwrap();
    assert!(matches!(write_round(&ib, &round, &done), Written::Yes(_)));
    assert_eq!(
        std::fs::read_to_string(results.join("result.md")).unwrap(),
        "mine"
    );
    // Item 2: the round's prompt is kept next to the reply.
    assert_eq!(
        std::fs::read_to_string(results.join("prompt.md")).unwrap(),
        "the prompt"
    );
}

/// Items A and C through the real write-back: the reply's local paths point
/// into the results folder in the tip and in `result.md`, and a deliverable
/// that was not copied is named on the tip (names only, no OS error, no
/// local path); the hash is over the final body.
#[test]
fn the_tip_and_result_md_cite_the_copies_and_name_what_was_not_copied() {
    use crate::inbox::block::Outcome;
    use crate::inbox::graphgen::NODE_ID;
    use crate::inbox::watch::{write_round, Round, Written};
    use crate::run_store::{node_run_dir, RESULT_FILE};
    use crate::store::deliverables_dir;

    let tips = tempfile::tempdir().unwrap();
    let (ib, data) = inbox(tips.path().to_path_buf(), 1);
    let tip = tips.path().join("t.md");
    std::fs::write(&tip, agent_tip_claimed_by("H", "r-mine")).unwrap();
    let graph_dir = crate::paths::abs(data.path())
        .join("inbox")
        .join("rounds")
        .join("t-1")
        .join("r1");
    let deliv = deliverables_dir(&graph_dir, NODE_ID);
    std::fs::create_dir_all(&deliv).unwrap();
    std::fs::write(deliv.join("answer.md"), "42").unwrap();
    let outside = data.path().join("outside.txt");
    std::fs::write(&outside, "x").unwrap();
    std::fs::hard_link(&outside, deliv.join("loot.md")).unwrap();
    let shown = crate::prompt::display_dir(&deliv);
    let reply = format!("Saved `{shown}/answer.md` and {shown}/loot.md.");
    let src = node_run_dir(&graph_dir, "g1", NODE_ID).join(RESULT_FILE);
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    std::fs::write(&src, &reply).unwrap();
    let round = Round {
        tip_id: "t".into(),
        path: tip.clone(),
        folder: tips.path().to_path_buf(),
        run: "r-mine".into(),
        graph_dir: graph_dir.clone(),
        graph_run: Some("g1".into()),
        expected_body: "Do x\n".into(),
        prompt: Some("p".into()),
    };
    let Written::Yes(body) = write_round(&ib, &round, &Outcome::Done { reply }) else {
        panic!("written");
    };
    let want = "Saved `agent/t/answer.md` and loot.md (not copied).";
    assert!(body.contains(want), "{body}");
    assert!(
        body.contains("Files: agent/t/\nNot copied to agent/t/: `loot.md`.\n"),
        "{body}"
    );
    let local = data.path().display().to_string();
    assert!(
        !body.contains(&local) && !body.contains("AppData"),
        "{body}"
    );
    assert!(
        !body.contains("hard link"),
        "no OS or runner error text: {body}"
    );
    let result = std::fs::read_to_string(tips.path().join("agent/t/result.md")).unwrap();
    assert_eq!(result, want);
    let parsed = crate::inbox::tip::parse("t", &std::fs::read_to_string(&tip).unwrap()).unwrap();
    assert_eq!(parsed.body, body);
    assert_eq!(
        parsed.agent.and_then(|a| a.hash()),
        Some(crate::inbox::hash::body_hash(&body))
    );
}

/// L10: the ended session is written into run.json (status untouched); a
/// write that has to wait (here: the folder is held by a run) is retried at
/// the next `close_finished`, so the record does not stay "alive" forever.
#[tokio::test(flavor = "multi_thread")]
async fn ended_sessions_reach_run_json_also_after_a_wait() {
    use crate::contract::{RunMode, RunState, RunStatus, SessionStatus, TerminalSession};
    use crate::run_store::{read_run_state, write_run_state};
    use crate::terminal::api::SessionShared;

    let tips = tempfile::tempdir().unwrap();
    let (ib, data) = inbox(tips.path().to_path_buf(), 1);
    let dir = crate::paths::abs(data.path()).join("g");
    std::fs::create_dir_all(&dir).unwrap();
    let session = |alive: bool| TerminalSession {
        sid: "s1".into(),
        title: "Board task".into(),
        node_ids: vec!["task".into()],
        status: SessionStatus::Done,
        pid: alive.then_some(4242),
        alive,
        current_node: None,
        harness: "claude".into(),
        pending_prompt: None,
    };
    let before = RunState {
        run_id: "g1".into(),
        graph_dir: dir.display().to_string(),
        started_at: chrono::Utc::now(),
        finished_at: Some(chrono::Utc::now()),
        status: RunStatus::Succeeded,
        nodes: Default::default(),
        mode: RunMode::Terminal,
        sessions: vec![session(true)],
    };
    write_run_state(&dir, &before).unwrap();
    // The session ended (its task gone, its own state says so).
    let shared = SessionShared::new("g1", &dir, session(false));
    ib.env().app.terminals().insert(shared);
    let alive = || read_run_state(&dir, "g1").unwrap().sessions[0].alive;
    assert!(alive(), "before the close the record says alive");

    // A run holds the folder: the write waits, it is remembered.
    ib.env()
        .app
        .with_dir_reserved(&dir, || ib.record_ended("t", "g1"))
        .unwrap();
    assert!(alive());
    assert!(ib.lock().unrecorded.contains_key("g1"));

    // The next close_finished writes it.
    ib.close_finished().await;
    let after = read_run_state(&dir, "g1").unwrap();
    assert!(!after.sessions[0].alive && after.sessions[0].pid.is_none());
    assert_eq!(after.status, RunStatus::Succeeded);
    assert!(ib.lock().unrecorded.is_empty());
}

/// LOW 10: a run whose sessions never end is not retried forever: after
/// `UNRECORDED_TRIES` tries it is dropped, logged once.
#[test]
fn an_unrecordable_run_is_dropped_after_a_bounded_number_of_tries() {
    use crate::contract::{SessionStatus, TerminalSession};
    use crate::terminal::api::SessionShared;

    let tips = tempfile::tempdir().unwrap();
    let (ib, data) = inbox(tips.path().to_path_buf(), 1);
    let dir = crate::paths::abs(data.path()).join("g");
    std::fs::create_dir_all(&dir).unwrap();
    let info = TerminalSession {
        sid: "s1".into(),
        title: "T".into(),
        node_ids: vec!["task".into()],
        status: SessionStatus::Done,
        pid: Some(1),
        alive: true,
        current_node: None,
        harness: "claude".into(),
        pending_prompt: None,
    };
    let shared = SessionShared::new("g9", &dir, info);
    shared.task_started(); // never ends
    ib.env().app.terminals().insert(shared);
    for _ in 0..crate::inbox::close::UNRECORDED_TRIES {
        ib.record_ended("t", "g9");
    }
    assert!(!ib.lock().unrecorded.contains_key("g9"));
    let dropped = ib
        .status()
        .log
        .iter()
        .filter(|l| {
            l.text
                .starts_with("gave up recording the ended session of run g9")
        })
        .count();
    assert_eq!(dropped, 1);
}

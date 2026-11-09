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

    // Still ours: copied and written.
    std::fs::write(&tip, agent_tip_claimed_by("H", "r-mine")).unwrap();
    assert!(matches!(write_round(&ib, &round, &done), Written::Yes(_)));
    assert_eq!(
        std::fs::read_to_string(results.join("result.md")).unwrap(),
        "mine"
    );
}

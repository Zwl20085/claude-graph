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

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_folder_schedules_a_bounded_number_of_rescans() {
    let tips = tempfile::tempdir().unwrap();
    let (ib, _data) = inbox(tips.path().to_path_buf(), 1);
    for _ in 0..(crate::inbox::retry::RESCAN_MAX + 3) {
        ib.schedule_rescan();
        // Only one pending at a time.
        ib.schedule_rescan();
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let log = ib.status().log;
    let scheduled = log
        .iter()
        .filter(|l| l.text.starts_with("re-scan in"))
        .count();
    assert_eq!(scheduled as u32, crate::inbox::retry::RESCAN_MAX);
    assert!(log.iter().any(|l| l.text.contains("retries used up")));
    assert!(log.iter().any(|l| l.text.starts_with("scanned:")));
}

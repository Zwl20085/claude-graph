//! The inbox loop end to end (docs/INBOX.md) in terminal mode, with the fake
//! CLI in a real pty: a temp tips folder, an injected host name, data dir
//! and settings. Covers: an agent tip runs and ends `done` with its result
//! files; a reply question ends `question` and leaves the session open; an
//! unchanged done tip is not run again; an edit makes it run again in a new
//! run; editing a question tip supersedes its open session; a `kind: task`
//! tip and a tip claimed by another host are left byte-identical; the
//! timeout ends a run as `failed`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use claude_graph_lib::contract::{InboxPhase, InboxStatus, InboxTipState};
use claude_graph_lib::inbox::graphgen::{inbox_root, round_dir, task_text, tip_dir};
use claude_graph_lib::inbox::hash::body_hash;
use claude_graph_lib::inbox::tip::{parse, Tip};
use claude_graph_lib::inbox::{Inbox, InboxEnv, NullSink, RunConfig};
use claude_graph_lib::paths::abs;
use claude_graph_lib::run_api::AppState;
use common::term::{received, wait_until};
use common::*;

const HOST: &str = "TESTHOST";

struct Rig {
    tips: tempfile::TempDir,
    app: AppState,
    inbox: Inbox,
    rec: Arc<Recorder>,
}

fn rig(timeout: Duration) -> Rig {
    use_fake_claude("");
    let tips = tempfile::tempdir().unwrap();
    let app = fresh_app();
    let rec = Arc::new(Recorder::default());
    let inbox = inbox_on(&app, tips.path(), timeout, rec.clone());
    Rig {
        tips,
        app,
        inbox,
        rec,
    }
}

/// An inbox on `app`'s data dir (a second one stands for an app restart).
fn inbox_on(app: &AppState, folder: &Path, timeout: Duration, rec: Arc<Recorder>) -> Inbox {
    let cfg = RunConfig {
        enabled: true,
        folder: folder.to_path_buf(),
        max_parallel: 2,
        timeout,
        interval: Duration::from_secs(12 * 3600),
    };
    Inbox::new(InboxEnv {
        app: app.clone(),
        emitter: rec,
        sink: Arc::new(NullSink),
        host: HOST.into(),
        clock: Arc::new(|| chrono::Local::now().naive_local()),
        config: Arc::new(move || cfg.clone()),
        suggest: Arc::new(|| None),
        poll: Duration::from_millis(100),
        retry_delay: Duration::from_millis(500),
    })
}

impl Rig {
    fn file(&self, id: &str) -> PathBuf {
        self.tips.path().join(format!("{id}.md"))
    }

    fn write(&self, id: &str, text: &str) {
        fs::write(self.file(id), text).unwrap();
    }

    fn tip(&self, id: &str) -> Tip {
        parse(id, &fs::read_to_string(self.file(id)).unwrap()).unwrap()
    }

    fn agent(&self, id: &str, key: &str) -> Option<String> {
        let t = self.tip(id);
        let a = t.agent.unwrap_or_default();
        match key {
            "state" => a.state_text(),
            "hash" => a.hash(),
            "host" => a.host(),
            "run" => a.run(),
            _ => a.raw.get(key).and_then(|v| v.as_str()).map(str::to_string),
        }
    }

    async fn scan(&self) -> InboxStatus {
        let st = self.inbox.scan_now().await.expect("scan");
        assert!(self.inbox.wait_idle(TIMEOUT).await, "inbox idle in time");
        st
    }

    fn status_tip(&self, id: &str) -> claude_graph_lib::contract::InboxTip {
        self.inbox
            .status()
            .tips
            .into_iter()
            .find(|t| t.id == id)
            .unwrap_or_else(|| panic!("tip {id} listed"))
    }

    /// The graph folder and graph run of the tip's last round (by its
    /// `agent.run`; a done round's entry is gone from the status).
    fn round(&self, id: &str) -> (PathBuf, String) {
        let run = self.agent(id, "run").expect("agent.run");
        let dir = round_dir(&inbox_root(self.app.data_dir()), id, &run);
        let runs: Vec<String> = fs::read_dir(dir.join("runs"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(runs.len(), 1, "one graph run per round folder: {runs:?}");
        (dir, runs[0].clone())
    }

    fn alive(&self, graph_run: &str) -> bool {
        self.app
            .terminals()
            .all_of(graph_run)
            .iter()
            .any(|s| s.is_task_alive())
    }

    async fn kill_all(&self) {
        self.app.terminals().kill_all();
        wait_until("sessions gone", || self.app.terminals().live_count() == 0).await;
    }
}

fn graph_run(st: &claude_graph_lib::contract::InboxTip) -> (PathBuf, String) {
    let run = st.run.as_ref().expect("run info");
    (
        PathBuf::from(&run.graph_dir),
        run.graph_run_id.clone().expect("graph run id"),
    )
}

const TASK: &str = "---\ntitle: Buy milk\nkind: task\nstatus: open\n---\nTwo litres.\n";

fn other_host_tip() -> String {
    format!(
        "---\ntitle: Theirs\nkind: agent\nagent:\n  v: 1\n  state: running\n  hash: {}\n  host: OTHER-PC\n  run: 20261109-070000-aaaa\n  at: 2026-11-09T07:00:00\n---\nTheir task.\n",
        body_hash("old")
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_inbox_loop_runs_tips_and_writes_outcomes_back() {
    let r = rig(Duration::from_secs(600));
    let alpha = "---\ntitle: Find papers\nkind: agent\ncolor: '#ff8800'\npaper:\n  doi: \"10.1/x\"\n---\nFind three recent papers on hairpin AC loss.\n";
    r.write("alpha", alpha);
    r.write(
        "quiz",
        "---\ntitle: Set up tests\nkind: agent\n---\nSet up the tests FAKE_QUESTION\n",
    );
    r.write("task", TASK);
    r.write("theirs", &other_host_tip());
    r.write(
        "perm",
        "---\ntitle: Make a folder\nkind: agent\n---\nMake it FAKE_PERMISSION\n",
    );
    let theirs = other_host_tip();

    r.scan().await;

    // A permission dialog in the window is reported as a question.
    assert_eq!(r.agent("perm", "state").as_deref(), Some("question"));
    let perm = r.tip("perm").body;
    assert!(perm.contains(&format!("Waiting at `{HOST}`: `")), "{perm}");
    assert!(
        perm.contains("Answer in the Claude Graph session window."),
        "{perm}"
    );

    // (a) done, with a result block, a matching hash and the files.
    assert_eq!(r.agent("alpha", "state").as_deref(), Some("done"));
    assert_eq!(r.agent("alpha", "host").as_deref(), Some(HOST));
    let tip = r.tip("alpha");
    assert!(tip
        .body
        .starts_with("Find three recent papers on hairpin AC loss.\n\n<!-- agent:begin "));
    assert!(
        tip.body.contains(" done -->\n## ⭐ Result · "),
        "{}",
        tip.body
    );
    assert!(tip
        .body
        .trim_end()
        .ends_with("Files: agent/alpha/\n<!-- agent:end -->"));
    assert_eq!(r.agent("alpha", "hash"), Some(body_hash(&tip.body)));
    assert_eq!(r.agent("alpha", "results").as_deref(), Some("agent/alpha/"));
    let result = r.tips.path().join("agent").join("alpha").join("result.md");
    assert!(fs::read_to_string(&result)
        .unwrap()
        .contains("ok (0 file(s) written)"));
    let text = fs::read_to_string(r.file("alpha")).unwrap();
    assert!(text.starts_with("---\ntitle: Find papers\nkind: agent\ncolor: '#ff8800'\npaper:\n  doi: \"10.1/x\"\nagent:\n"), "{text}");

    // Item 2: the prompt the round ran with is kept next to the reply.
    let alpha_body = "Find three recent papers on hairpin AC loss.\n";
    let prompt = r.tips.path().join("agent").join("alpha").join("prompt.md");
    assert_eq!(
        fs::read_to_string(&prompt).unwrap(),
        task_text("Find papers", alpha_body)
    );

    // Item 3: a done round's session is ended and its window closed right
    // after the write-back; its entry is gone.
    let (gdir, grun) = r.round("alpha");
    let label = format!("term-{grun}-s1");
    wait_until("the done window to be closed", || {
        r.rec.closed.lock().unwrap().contains(&label)
    })
    .await;
    wait_until("the done entry to be dropped", || {
        r.status_tip("alpha").run.is_none()
    })
    .await;
    assert!(!r.alive(&grun));
    assert_eq!(r.agent("alpha", "state").as_deref(), Some("done"));

    // The tip text was never typed: only the pointer line to the brief.
    let typed = received(&gdir, &grun, "s1");
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert!(
        typed[0].starts_with("Node \"Board task\" (task): read the brief at "),
        "{typed:?}"
    );
    assert!(!typed[0].contains("hairpin"));
    // The block carries the reply, not the echo of the pointer line.
    assert!(!r.tip("alpha").body.contains("> Node \"Board task\""));
    assert!(gdir.starts_with(r.app.data_dir()));

    // (b) a reply question: state question, a questions block, session left open.
    assert_eq!(r.agent("quiz", "state").as_deref(), Some("question"));
    let quiz = r.tip("quiz");
    assert!(
        quiz.body.contains(" question -->\n## ❓ Questions · "),
        "{}",
        quiz.body
    );
    assert!(quiz.body.contains("Which framework should I use?"));
    let qst = r.status_tip("quiz");
    assert_eq!(qst.state, InboxTipState::Question);
    assert_eq!(qst.run.as_ref().unwrap().phase, InboxPhase::Question);
    let (_, quiz_run) = graph_run(&qst);
    assert!(r
        .app
        .terminals()
        .all_of(&quiz_run)
        .iter()
        .any(|s| s.is_task_alive()));

    // (e) a task tip and (f) another host's claim are byte-identical.
    assert_eq!(fs::read_to_string(r.file("task")).unwrap(), TASK);
    assert_eq!(fs::read_to_string(r.file("theirs")).unwrap(), theirs);
    assert_eq!(r.status_tip("theirs").decision, "running on OTHER-PC");
    assert_eq!(r.inbox.status().other_tips, 1);

    // (c) an unchanged done tip is not run again.
    let first_run = r.agent("alpha", "run").unwrap();
    let before = fs::read_to_string(r.file("alpha")).unwrap();
    r.scan().await;
    assert_eq!(fs::read_to_string(r.file("alpha")).unwrap(), before);
    assert!(r
        .status_tip("alpha")
        .decision
        .starts_with("unchanged since done"));

    // Item 4: Claude Code wrote its own settings into the round's folder
    // (the owner answered a dialog); the next round is not refused for it,
    // and the file is left as it is.
    let local = gdir.join(".claude").join("settings.local.json");
    fs::create_dir_all(local.parent().unwrap()).unwrap();
    fs::write(&local, "{\"enabledMcpjsonServers\":[]}").unwrap();

    // (d) appended text makes it eligible: a new run, a second block.
    fs::write(r.file("alpha"), format!("{before}\nAlso include arXiv.\n")).unwrap();
    r.scan().await;
    assert_eq!(
        r.agent("alpha", "state").as_deref(),
        Some("done"),
        "{}",
        r.status_tip("alpha").decision
    );
    let second_run = r.agent("alpha", "run").unwrap();
    assert_ne!(first_run, second_run);
    let (gdir2, _) = r.round("alpha");
    assert_ne!(gdir2, gdir, "a fresh folder per round");
    assert_eq!(gdir2.parent(), gdir.parent());
    assert_eq!(
        gdir.parent().unwrap(),
        tip_dir(&inbox_root(r.app.data_dir()), "alpha")
    );
    assert!(!gdir2.join(".claude").exists());
    assert!(local.is_file(), "never deleted");
    assert!(fs::read_to_string(&prompt)
        .unwrap()
        .contains("Also include arXiv."));
    let tip = r.tip("alpha");
    let before_body = parse("alpha", &before).unwrap().body;
    assert!(tip.body.starts_with(&format!(
        "{before_body}\nAlso include arXiv.\n\n<!-- agent:begin"
    )));
    assert_eq!(tip.body.matches("## ⭐ Result").count(), 2);
    assert_eq!(r.agent("alpha", "hash"), Some(body_hash(&tip.body)));

    // An answer typed into the question tip supersedes its open session.
    let quiz_text = fs::read_to_string(r.file("quiz")).unwrap();
    fs::write(r.file("quiz"), format!("{quiz_text}\nUse vitest.\n")).unwrap();
    r.scan().await;
    wait_until("the earlier quiz session to end", || {
        !r.app
            .terminals()
            .all_of(&quiz_run)
            .iter()
            .any(|s| s.is_task_alive())
    })
    .await;
    let (_, new_quiz_run) = graph_run(&r.status_tip("quiz"));
    assert_ne!(new_quiz_run, quiz_run);
    assert_eq!(r.tip("quiz").body.matches("## ❓ Questions").count(), 2);

    assert_eq!(fs::read_to_string(r.file("task")).unwrap(), TASK);
    assert_eq!(fs::read_to_string(r.file("theirs")).unwrap(), theirs);
    assert!(no_temp_files(r.tips.path()));
    r.kill_all().await;
}

fn no_temp_files(dir: &Path) -> bool {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_over_the_timeout_is_cancelled_and_reported_failed() {
    let r = rig(Duration::from_secs(3));
    r.write(
        "slow",
        "---\ntitle: Slow\nkind: agent\n---\nTake your time FAKE_SLEEP_MS=60000\n",
    );
    r.scan().await;
    assert_eq!(r.agent("slow", "state").as_deref(), Some("failed"));
    let tip = r.tip("slow");
    assert!(tip.body.contains("## ⚠ Failed · "), "{}", tip.body);
    assert!(tip.body.contains("timed out after 3 s"), "{}", tip.body);
    assert_eq!(r.agent("slow", "hash"), Some(body_hash(&tip.body)));
    r.kill_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_inbox_does_not_scan() {
    use_fake_claude("");
    let tips = tempfile::tempdir().unwrap();
    fs::write(tips.path().join("a.md"), "---\nkind: agent\n---\nx\n").unwrap();
    let cfg = RunConfig {
        enabled: false,
        folder: tips.path().to_path_buf(),
        max_parallel: 1,
        timeout: Duration::from_secs(60),
        interval: Duration::from_secs(3600),
    };
    let env = InboxEnv {
        app: fresh_app(),
        emitter: Arc::new(Recorder::default()),
        sink: Arc::new(NullSink),
        host: HOST.into(),
        clock: Arc::new(|| chrono::Local::now().naive_local()),
        config: Arc::new(move || cfg.clone()),
        suggest: Arc::new(|| Some("C:/suggested".into())),
        poll: Duration::from_millis(100),
        retry_delay: Duration::from_millis(500),
    };
    let inbox = Inbox::new(env);
    assert_eq!(inbox.scan_now().await.unwrap_err().code, "invalid_input");
    assert_eq!(
        fs::read_to_string(tips.path().join("a.md")).unwrap(),
        "---\nkind: agent\n---\nx\n"
    );
    assert!(inbox.status().tips.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn at_most_max_parallel_sessions_work_and_the_rest_wait_unclaimed() {
    use_fake_claude("");
    let tips = tempfile::tempdir().unwrap();
    for id in ["one", "two", "three"] {
        fs::write(
            tips.path().join(format!("{id}.md")),
            format!("---\ntitle: {id}\nkind: agent\n---\nWork FAKE_SLEEP_MS=1500 on {id}\n"),
        )
        .unwrap();
    }
    let cfg = RunConfig {
        enabled: true,
        folder: tips.path().to_path_buf(),
        max_parallel: 1,
        timeout: Duration::from_secs(120),
        interval: Duration::from_secs(3600),
    };
    let app = fresh_app();
    let env = InboxEnv {
        app: app.clone(),
        emitter: Arc::new(Recorder::default()),
        sink: Arc::new(NullSink),
        host: HOST.into(),
        clock: Arc::new(|| chrono::Local::now().naive_local()),
        config: Arc::new(move || cfg.clone()),
        suggest: Arc::new(|| None),
        poll: Duration::from_millis(100),
        retry_delay: Duration::from_millis(500),
    };
    let inbox = Inbox::new(env);
    let first = inbox.scan_now().await.unwrap();
    let phases: Vec<Option<InboxPhase>> = first
        .tips
        .iter()
        .map(|t| t.run.as_ref().map(|r| r.phase))
        .collect();
    assert_eq!(
        phases
            .iter()
            .filter(|p| **p == Some(InboxPhase::Working))
            .count(),
        1,
        "{phases:?}"
    );
    assert_eq!(
        phases
            .iter()
            .filter(|p| **p == Some(InboxPhase::Waiting))
            .count(),
        2,
        "{phases:?}"
    );
    // A waiting tip is not claimed yet.
    let claimed = |id: &str| {
        fs::read_to_string(tips.path().join(format!("{id}.md")))
            .unwrap()
            .contains("agent:")
    };
    assert_eq!(
        ["one", "two", "three"]
            .iter()
            .filter(|id| claimed(id))
            .count(),
        1
    );
    // A scan request during a scan is coalesced, never overlapping.
    let (a, b) = tokio::join!(inbox.scan_now(), inbox.scan_now());
    assert!(a.is_ok() && b.is_ok());
    let mut max_seen = 0;
    let until = std::time::Instant::now() + TIMEOUT;
    while !inbox.wait_idle(Duration::from_millis(50)).await {
        let working = inbox
            .status()
            .tips
            .iter()
            .filter(|t| {
                t.run
                    .as_ref()
                    .is_some_and(|r| r.phase == InboxPhase::Working)
            })
            .count();
        max_seen = max_seen.max(working);
        assert!(std::time::Instant::now() < until, "idle in time");
    }
    assert_eq!(max_seen, 1);
    for id in ["one", "two", "three"] {
        let t = parse(
            id,
            &fs::read_to_string(tips.path().join(format!("{id}.md"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            t.agent.unwrap().state_text().as_deref(),
            Some("done"),
            "{id}"
        );
    }
    app.terminals().kill_all();
    wait_until("sessions gone", || app.terminals().live_count() == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_claims_rerun_only_for_recorded_runs_and_planted_files_block() {
    let r = rig(Duration::from_secs(600));
    // A claim of this host that this machine never recorded (a OneDrive
    // conflict copy of a running tip, a hand edit): left alone.
    let ghost = "---\ntitle: Ghost\nkind: agent\nagent:\n  v: 1\n  state: running\n  host: TESTHOST\n  run: 20260101-000000-dead\n---\nOld task.\n";
    r.write("mine-TESTHOST", ghost);
    r.write("mine", "---\ntitle: Mine\nkind: agent\n---\nDo mine.\n");
    r.scan().await;
    assert_eq!(fs::read_to_string(r.file("mine-TESTHOST")).unwrap(), ghost);
    assert_eq!(
        r.status_tip("mine-TESTHOST").decision,
        claude_graph_lib::inbox::eligible::UNKNOWN_RUN
    );
    assert_eq!(r.agent("mine", "state").as_deref(), Some("done"));
    let first_run = r.agent("mine", "run").unwrap();
    let (gdir, _) = r.round("mine");
    r.kill_all().await;

    // A crash left `mine` running with a run id this machine recorded: after
    // a restart (a new inbox on the same data dir) it is re-run as stale.
    let crashed = fs::read_to_string(r.file("mine"))
        .unwrap()
        .replace("state: done", "state: running");
    fs::write(r.file("mine"), &crashed).unwrap();
    let restarted = inbox_on(
        &r.app,
        r.tips.path(),
        Duration::from_secs(600),
        Arc::new(Recorder::default()),
    );
    restarted.scan_now().await.unwrap();
    assert!(restarted.wait_idle(TIMEOUT).await);
    assert_eq!(r.agent("mine", "state").as_deref(), Some("done"));
    assert_ne!(r.agent("mine", "run").unwrap(), first_run);
    assert_eq!(fs::read_to_string(r.file("mine-TESTHOST")).unwrap(), ghost);
    r.app.terminals().kill_all();
    wait_until("sessions gone", || r.app.terminals().live_count() == 0).await;

    // A file an earlier round left in its own folder is not in the next
    // round's folder or above it; one planted in a parent (here the tip's
    // folder of rounds) is: the next round does not start, the tip is not
    // written, the reason names that folder only.
    let tip_rounds = gdir.parent().unwrap().to_path_buf();
    fs::write(gdir.join("CLAUDE.md"), "planted").unwrap();
    fs::write(tip_rounds.join("CLAUDE.md"), "planted").unwrap();
    let before = format!(
        "{}\nMore please.\n",
        fs::read_to_string(r.file("mine")).unwrap()
    );
    fs::write(r.file("mine"), &before).unwrap();
    restarted.scan_now().await.unwrap();
    assert!(restarted.wait_idle(TIMEOUT).await);
    assert_eq!(fs::read_to_string(r.file("mine")).unwrap(), before);
    let tip = restarted
        .status()
        .tips
        .into_iter()
        .find(|t| t.id == "mine")
        .unwrap();
    assert!(
        tip.decision.starts_with("not started: "),
        "{}",
        tip.decision
    );
    assert!(
        tip.decision.contains(&format!(
            "{} contains CLAUDE.md",
            abs(&tip_rounds).display()
        )),
        "{}",
        tip.decision
    );
    assert!(
        !tip.decision.contains(&abs(&gdir).display().to_string()),
        "{}",
        tip.decision
    );
    assert!(gdir.join("CLAUDE.md").is_file(), "never deleted");
    assert!(tip_rounds.join("CLAUDE.md").is_file(), "never deleted");
}

fn set_read_only(path: &Path, on: bool) {
    let mut p = fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    p.set_readonly(on);
    fs::set_permissions(path, p).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outcome_for_a_read_only_tip_is_kept_and_written_at_the_next_scan() {
    let r = rig(Duration::from_secs(600));
    r.write(
        "ro",
        "---\ntitle: RO\nkind: agent\n---\nWork FAKE_SLEEP_MS=2500 please\n",
    );
    r.inbox.scan_now().await.unwrap();
    wait_until("the claim", || {
        r.agent("ro", "state").as_deref() == Some("running")
    })
    .await;
    let run = r.agent("ro", "run").unwrap();
    set_read_only(&r.file("ro"), true);
    assert!(r.inbox.wait_idle(TIMEOUT).await);
    wait_until("the kept outcome", || {
        r.status_tip("ro")
            .decision
            .starts_with("outcome not written yet: tip file is read-only")
    })
    .await;
    assert_eq!(r.agent("ro", "state").as_deref(), Some("running"));
    // Still read-only: the next scan neither re-runs it nor loses the outcome.
    r.scan().await;
    assert_eq!(r.agent("ro", "run").as_deref(), Some(run.as_str()));
    assert!(r
        .status_tip("ro")
        .decision
        .starts_with("outcome not written yet"));
    set_read_only(&r.file("ro"), false);
    r.scan().await;
    assert_eq!(r.agent("ro", "state").as_deref(), Some("done"));
    assert_eq!(
        r.agent("ro", "run").as_deref(),
        Some(run.as_str()),
        "written, not re-run"
    );
    assert_eq!(r.tip("ro").body.matches("<!-- agent:end -->").count(), 1);
    r.kill_all().await;
}

/// Item 3: a done round's session is ended and its window closed right
/// after its outcome and results are written (no scan needed); a question
/// session stays open, also through the next scan.
#[tokio::test(flavor = "multi_thread")]
async fn done_sessions_close_after_the_write_back_and_questions_stay_open() {
    let r = rig(Duration::from_secs(600));
    r.write("done", "---\ntitle: Done\nkind: agent\n---\nDo it.\n");
    r.write(
        "asks",
        "---\ntitle: Asks\nkind: agent\n---\nSet up the tests FAKE_QUESTION\n",
    );
    r.scan().await;
    assert_eq!(r.agent("done", "state").as_deref(), Some("done"));
    assert_eq!(r.agent("asks", "state").as_deref(), Some("question"));
    let (_, done_run) = r.round("done");
    let (_, asks_run) = graph_run(&r.status_tip("asks"));
    let closed_line = format!("closed the done session (run {done_run})");
    wait_until("the done session to be closed", || {
        r.inbox.status().log.iter().any(|l| l.text == closed_line)
    })
    .await;
    assert!(!r.alive(&done_run));
    assert!(r.tips.path().join("agent/done/result.md").is_file());
    assert!(r.tips.path().join("agent/done/prompt.md").is_file());
    let closed = r.rec.closed.lock().unwrap().clone();
    assert_eq!(
        closed,
        [format!("term-{done_run}-s1")],
        "only the done window"
    );
    assert!(r.status_tip("done").run.is_none(), "its entry is dropped");
    let before = fs::read_to_string(r.file("done")).unwrap();

    r.scan().await;
    assert!(
        r.alive(&asks_run),
        "a question session is not touched: {:#?}",
        r.inbox.status().log
    );
    assert_eq!(
        r.status_tip("asks").run.as_ref().map(|x| x.phase),
        Some(InboxPhase::Question)
    );
    assert_eq!(fs::read_to_string(r.file("done")).unwrap(), before);
    assert_eq!(r.rec.closed.lock().unwrap().len(), 1);
    r.kill_all().await;
}

/// The owner edits the tip while its round works: the outcome is written
/// with the hash of the seen body, the tip shows queued, and the next scan
/// runs it again with the edit.
#[tokio::test(flavor = "multi_thread")]
async fn an_edit_during_the_run_is_written_with_the_seen_hash_and_runs_again() {
    let r = rig(Duration::from_secs(600));
    let seen = "Work FAKE_SLEEP_MS=2500 please\n";
    r.write("ed", &format!("---\ntitle: Ed\nkind: agent\n---\n{seen}"));
    r.inbox.scan_now().await.unwrap();
    wait_until("the claim", || {
        r.agent("ed", "state").as_deref() == Some("running")
    })
    .await;
    let first_run = r.agent("ed", "run").unwrap();
    let claimed = fs::read_to_string(r.file("ed")).unwrap();
    fs::write(r.file("ed"), format!("{claimed}\nAlso add a summary.\n")).unwrap();
    assert!(r.inbox.wait_idle(TIMEOUT).await);
    wait_until("the outcome", || {
        r.agent("ed", "state").as_deref() == Some("done")
    })
    .await;
    let tip = r.tip("ed");
    assert!(
        tip.body
            .starts_with(&format!("{seen}\nAlso add a summary.\n\n<!-- agent:begin")),
        "{}",
        tip.body
    );
    assert_eq!(r.agent("ed", "hash"), Some(body_hash(seen)));
    let st = r.status_tip("ed");
    assert_eq!(st.state, InboxTipState::Queued);
    assert!(st.eligible, "{}", st.decision);

    r.scan().await;
    assert_eq!(r.agent("ed", "state").as_deref(), Some("done"));
    assert_ne!(r.agent("ed", "run").unwrap(), first_run);
    let tip = r.tip("ed");
    assert_eq!(tip.body.matches("## ⭐ Result").count(), 2);
    assert_eq!(r.agent("ed", "hash"), Some(body_hash(&tip.body)));
    assert_eq!(r.status_tip("ed").state, InboxTipState::Done);
    r.kill_all().await;
}

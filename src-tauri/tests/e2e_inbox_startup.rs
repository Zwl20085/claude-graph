//! Item 1 end to end: a Claude Code start-up dialog (the fake CLI's
//! workspace-trust dialog, `--fake-dialog-ms`) holds an Inbox session before
//! its message is typed. The tip shows ❓ with the fixed dialog text while it
//! waits (written once), nothing is typed into the dialog, the time at the
//! dialog does not count toward the timeout, and when the dialog goes away
//! (the fake closes it itself, standing for the owner's answer) the round
//! finishes and `done` is written as one more block. A session that dies at
//! the dialog ends the round `failed` (the watcher never waits forever).
//!
//! Its own test binary: the fake CLI's switch is set through the process
//! environment, which every test of a binary shares.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use claude_graph_lib::contract::{InboxPhase, NodeEventKind};
use claude_graph_lib::inbox::hash::body_hash;
use claude_graph_lib::inbox::startup::TRUST_TEXT;
use claude_graph_lib::inbox::tip::{parse, Tip};
use claude_graph_lib::inbox::{Inbox, InboxEnv, NullSink, RunConfig};
use claude_graph_lib::run_api::AppState;
use common::term::{received, wait_until};
use common::*;

const HOST: &str = "TESTHOST";
/// How long the fake's start-up dialog stays open …
const DIALOG_MS: u64 = 6000;
/// … longer than the round's timeout: time at the dialog must not count.
const TIMEOUT_S: u64 = 4;

struct Rig {
    _tips: tempfile::TempDir,
    file: PathBuf,
    app: AppState,
    rec: Arc<Recorder>,
    inbox: Inbox,
}

fn rig(id: &str) -> Rig {
    use_fake_claude(&format!("--fake-dialog-ms={DIALOG_MS}"));
    let tips = tempfile::tempdir().unwrap();
    let file = tips.path().join(format!("{id}.md"));
    fs::write(&file, "---\ntitle: Trusty\nkind: agent\n---\nDo it.\n").unwrap();
    let cfg = RunConfig {
        enabled: true,
        folder: tips.path().to_path_buf(),
        max_parallel: 1,
        timeout: Duration::from_secs(TIMEOUT_S),
        interval: Duration::from_secs(3600),
    };
    let app = fresh_app();
    let rec = Arc::new(Recorder::default());
    let inbox = Inbox::new(InboxEnv {
        app: app.clone(),
        emitter: rec.clone(),
        sink: Arc::new(NullSink),
        host: HOST.into(),
        clock: Arc::new(|| chrono::Local::now().naive_local()),
        config: Arc::new(move || cfg.clone()),
        suggest: Arc::new(|| None),
        poll: Duration::from_millis(100),
        retry_delay: Duration::from_millis(500),
    });
    Rig {
        _tips: tips,
        file,
        app,
        rec,
        inbox,
    }
}

impl Rig {
    fn tip(&self, id: &str) -> Tip {
        parse(id, &fs::read_to_string(&self.file).unwrap()).unwrap()
    }

    fn state(&self, id: &str) -> Option<String> {
        self.tip(id).agent.and_then(|a| a.state_text())
    }

    fn phase(&self) -> Option<InboxPhase> {
        self.inbox
            .status()
            .tips
            .first()?
            .run
            .as_ref()
            .map(|r| r.phase)
    }

    /// Wait for the ❓ block of the trust dialog; returns (graph dir, graph run).
    async fn at_the_dialog(&self, id: &str) -> (PathBuf, String) {
        self.inbox.scan_now().await.unwrap();
        wait_until("the question block", || {
            self.state(id).as_deref() == Some("question")
        })
        .await;
        let body = self.tip(id).body;
        assert!(
            body.contains(&format!(
                "Waiting at `{HOST}`: `{TRUST_TEXT}`. Answer in the Claude Graph session window."
            )),
            "{body}"
        );
        wait_until("the question phase", || {
            self.phase() == Some(InboxPhase::Question)
        })
        .await;
        let run = self.inbox.status().tips[0].run.clone().expect("a run");
        (
            PathBuf::from(&run.graph_dir),
            run.graph_run_id.expect("graph run"),
        )
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_start_up_dialog_shows_as_a_question_then_the_round_finishes() {
    let r = rig("trusty");
    let (dir, graph_run) = r.at_the_dialog("trusty").await;
    // Nothing was typed into the dialog.
    assert!(received(&dir, &graph_run, "s1").is_empty());
    // The session's own notification fires for this dialog. It is the
    // session's event and the tip is the watcher's write; they are
    // independent, so the notification may land up to one session poll
    // after the tip shows the question: wait for it.
    wait_until("the dialog's notification", || {
        r.rec
            .events_of("task", NodeEventKind::Notification)
            .iter()
            .any(|t| t.starts_with("Claude Code is asking something in the window"))
    })
    .await;

    // The dialog goes away after DIALOG_MS (> the timeout): the round works
    // on and finishes; it did not time out at the dialog.
    wait_until("the outcome", || {
        matches!(r.state("trusty").as_deref(), Some("done" | "failed"))
    })
    .await;
    let body = r.tip("trusty").body;
    assert_eq!(r.state("trusty").as_deref(), Some("done"), "{body}");
    assert_eq!(
        body.matches("## ❓ Questions").count(),
        1,
        "written once: {body}"
    );
    assert_eq!(body.matches("## ⭐ Result").count(), 1, "{body}");
    assert!(
        body.find("## ❓ Questions") < body.find("## ⭐ Result"),
        "{body}"
    );
    assert_eq!(
        r.tip("trusty").agent.and_then(|a| a.hash()),
        Some(body_hash(&body)),
        "the body as the run left it"
    );
    // The pointer line was typed once, after the dialog.
    let typed = received(&dir, &graph_run, "s1");
    assert_eq!(typed.len(), 1, "{typed:?}");
    wait_until("the done session to end", || {
        !r.app
            .terminals()
            .all_of(&graph_run)
            .iter()
            .any(|s| s.is_task_alive())
    })
    .await;
    r.app.terminals().kill_all();
}

/// C1: the session dies while the dialog is on its (last) screen: the round
/// ends `failed` instead of waiting at a dead session forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_that_dies_at_the_dialog_ends_the_round_failed() {
    let r = rig("dies");
    let (_, graph_run) = r.at_the_dialog("dies").await;
    r.app.terminals().kill_run(&graph_run);
    wait_until("the failed block", || {
        r.state("dies").as_deref() == Some("failed")
    })
    .await;
    let body = r.tip("dies").body;
    assert_eq!(body.matches("## ⚠ Failed").count(), 1, "{body}");
    assert_eq!(r.phase(), Some(InboxPhase::Finished));
    r.app.terminals().kill_all();
}

//! Item 1 end to end: a Claude Code start-up dialog (the fake CLI's
//! workspace-trust dialog, `--fake-dialog-ms`) holds an Inbox session before
//! its message is typed. The tip shows ❓ with the fixed dialog text while it
//! waits (written once), nothing is typed into the dialog, and when the
//! dialog goes away (the fake closes it itself, standing for the owner's
//! answer) the round finishes and `done` is written as one more block.
//!
//! Its own test binary: the fake CLI's switch is set through the process
//! environment, which every test of a binary shares.

mod common;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use claude_graph_lib::contract::InboxPhase;
use claude_graph_lib::inbox::hash::body_hash;
use claude_graph_lib::inbox::startup::TRUST_TEXT;
use claude_graph_lib::inbox::tip::parse;
use claude_graph_lib::inbox::{Inbox, InboxEnv, NullSink, RunConfig};
use common::term::wait_until;
use common::*;

const HOST: &str = "TESTHOST";
/// How long the fake's start-up dialog stays open.
const DIALOG_MS: u64 = 6000;

#[tokio::test(flavor = "multi_thread")]
async fn a_start_up_dialog_shows_as_a_question_then_the_round_finishes() {
    use_fake_claude(&format!("--fake-dialog-ms={DIALOG_MS}"));
    let tips = tempfile::tempdir().unwrap();
    let file = tips.path().join("trusty.md");
    fs::write(&file, "---\ntitle: Trusty\nkind: agent\n---\nDo it.\n").unwrap();
    let cfg = RunConfig {
        enabled: true,
        folder: tips.path().to_path_buf(),
        max_parallel: 1,
        timeout: Duration::from_secs(600),
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
    let tip = || parse("trusty", &fs::read_to_string(&file).unwrap()).unwrap();
    let state = || tip().agent.and_then(|a| a.state_text());
    inbox.scan_now().await.unwrap();

    // While the dialog is open: ❓ with the dialog named, phase question.
    wait_until("the question block", || {
        state().as_deref() == Some("question")
    })
    .await;
    let body = tip().body;
    assert!(
        body.contains(&format!(
            "Waiting at `{HOST}`: `{TRUST_TEXT}`. Answer in the Claude Graph session window."
        )),
        "{body}"
    );
    let st = inbox.status();
    let run = st.tips[0].run.clone().expect("a run");
    assert_eq!(run.phase, InboxPhase::Question);
    let graph_run = run.graph_run_id.expect("graph run");
    // Nothing was typed into the dialog.
    let dir = std::path::PathBuf::from(&run.graph_dir);
    assert!(common::term::received(&dir, &graph_run, "s1").is_empty());
    // The session's own notification fired for the dialog.
    assert!(rec
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e.kind == claude_graph_lib::contract::NodeEventKind::Notification));

    // The dialog goes away: the round works on and finishes.
    wait_until("the done block", || state().as_deref() == Some("done")).await;
    let body = tip().body;
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
        tip().agent.and_then(|a| a.hash()),
        Some(body_hash(&body)),
        "the body as the run left it"
    );
    // The pointer line was typed once, after the dialog.
    let typed = common::term::received(&dir, &graph_run, "s1");
    assert_eq!(typed.len(), 1, "{typed:?}");
    wait_until("the done session to end", || {
        !app.terminals()
            .all_of(&graph_run)
            .iter()
            .any(|s| s.is_task_alive())
    })
    .await;
    app.terminals().kill_all();
}

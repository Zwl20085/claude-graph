//! The DeepSeek Harness CLI `dsh` for real (the owner's own configuration
//! in ~/.dsh; never read by the app). Ignored by default; two tiny turns:
//!
//! `cargo test --test real_dsh -- --ignored --nocapture`
//!
//! Node a asks dsh to remember a word, node b (same session, adopted with
//! `--session-id`) asks for it back: b's reply must hold the word. Also
//! checks `check_harness` (`dsh --version` through the shim resolution).

mod common;

use std::sync::Arc;

use claude_graph_lib::contract::{RunStatus, RunTarget};
use claude_graph_lib::harness::command::ENV_DSH_BIN;
use common::term::*;
use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "runs the real dsh CLI (two tiny DeepSeek turns)"]
async fn real_dsh_chains_two_nodes() {
    std::env::remove_var(ENV_DSH_BIN);
    let profile = claude_graph_lib::harness::profile::builtins()
        .into_iter()
        .find(|p| p.id == "deepseek-harness")
        .unwrap();
    let check = claude_graph_lib::harness::check(&profile, None).await;
    println!("check_harness: {check:?}");
    assert!(check.ok, "{check:?}");

    let nodes = vec![
        node(
            "a",
            0.0,
            0.0,
            json!({ "prompt": "Remember the word PELICAN. Reply with the single word ok." }),
        ),
        node(
            "b",
            0.0,
            100.0,
            json!({ "prompt": "Which word did I ask you to remember? Reply with that word only." }),
        ),
    ];
    let mut s = settings(1, false);
    s["harness"] = json!("deepseek-harness");
    let tmp = graph_dir(&graph(nodes, &[("a", "b")], s));
    let dir = tmp.path();
    let app = trusted_app(dir);
    let rec = Arc::new(Recorder::default());
    let (run_id, state) = run(&app, dir, RunTarget::All, &rec).await;
    for id in ["a", "b"] {
        let reply =
            std::fs::read_to_string(dir.join("runs").join(&run_id).join(id).join("reply.md"))
                .unwrap_or_default();
        println!("--- {id} reply.md ---\n{reply}");
    }
    let window: String = rec
        .pty
        .lock()
        .unwrap()
        .iter()
        .map(|(_, d)| d.data.clone())
        .collect();
    println!("--- window ---\n{window}");
    println!("run {run_id}: {:?}", state.status);
    assert_eq!(state.status, RunStatus::Succeeded, "{state:#?}");
    let b = std::fs::read_to_string(dir.join("runs").join(&run_id).join("b/reply.md")).unwrap();
    assert!(b.to_uppercase().contains("PELICAN"), "{b}");
    kill_sessions(&app, &run_id).await;
}

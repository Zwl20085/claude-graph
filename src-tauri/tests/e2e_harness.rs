//! End-to-end: harness profiles in terminal mode, through
//! scripts/fake-claude.mjs and scripts/fake-codex.mjs in real ptys, with the
//! built app binary as Codex's notify program. Needs `node`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use claude_graph_lib::claude_bin::find_on_path;
use claude_graph_lib::contract::{
    HarnessKind, HarnessProfile, NodeStatus, RunStatus, RunTarget, SessionStatus, StopMode,
};
use claude_graph_lib::harness::command::ENV_CODEX_BIN;
use claude_graph_lib::run_api::AppState;
use common::term::*;
use common::*;
use serde_json::{json, Value};

const APP_EXE: &str = env!("CARGO_BIN_EXE_claude-graph");

fn script(name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join(name)
        .canonicalize()
        .expect("script exists");
    p.display()
        .to_string()
        .trim_start_matches(r"\\?\")
        .to_string()
}

/// Fake claude and fake codex for every test of this binary (same values).
fn use_fakes() {
    use_fake_claude("");
    std::env::set_var(
        ENV_CODEX_BIN,
        format!("node \"{}\"", script("fake-codex.mjs")),
    );
}

fn app_for(dir: &Path) -> AppState {
    trusted_app(dir).with_notify_exe(PathBuf::from(APP_EXE))
}

fn session_json(dir: &Path, run_id: &str, sid: &str) -> Value {
    let path = dir
        .join("runs")
        .join(run_id)
        .join("sessions")
        .join(sid)
        .join("session.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn node_path() -> String {
    find_on_path(
        "node",
        std::env::var_os("PATH").as_ref(),
        std::env::var("PATHEXT").ok().as_deref(),
    )
    .expect("node on PATH")
    .display()
    .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chain_switches_harness_in_order_with_a_brief_between_sessions() {
    use_fakes();
    let g = graph(
        vec![
            node("survey", 0.0, 0.0, json!({ "prompt": "survey the field" })),
            node(
                "design",
                0.0,
                100.0,
                json!({ "prompt": "design it", "harness": "codex", "model": "opus",
                        "deliverables": [{ "path": "design.md" }] }),
            ),
            node(
                "report",
                0.0,
                200.0,
                json!({ "prompt": "write the report" }),
            ),
        ],
        &[("survey", "design"), ("design", "report")],
        settings(2, false),
    );
    let tmp = graph_dir(&g);
    let dir = tmp.path();
    let app = app_for(dir);
    let rec = Arc::new(Recorder::default());
    let (run_id, state) = run(&app, dir, RunTarget::All, &rec).await;

    assert_eq!(state.status, RunStatus::Succeeded, "{state:#?}");
    let sessions: Vec<(&str, &str, &str)> = state
        .sessions
        .iter()
        .map(|s| (s.sid.as_str(), s.title.as_str(), s.harness.as_str()))
        .collect();
    assert_eq!(
        sessions,
        [
            ("s1", "T survey", "claude"),
            ("s2", "T design (Codex CLI)", "codex"),
            ("s3", "T report", "claude"),
        ]
    );
    assert!(state
        .sessions
        .iter()
        .all(|s| s.status == SessionStatus::Done));
    // In order: each session started after the previous one's node ended.
    let started = |sid: &str| -> String {
        session_json(dir, &run_id, sid)["started_at"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let ended = |id: &str| state.nodes[id].finished_at.unwrap();
    let parse = |s: String| s.parse::<chrono::DateTime<chrono::Utc>>().unwrap();
    assert!(ended("survey") <= parse(started("s2")));
    assert!(ended("design") <= parse(started("s3")));
    assert_eq!(session_json(dir, &run_id, "s2")["harness"], "codex");

    // The design node: the one-line brief form, upstream reply in brief.md.
    let s2 = received(dir, &run_id, "s2");
    assert_eq!(s2.len(), 1, "{s2:?}");
    assert!(
        s2[0].starts_with("Node \"T design\" (design): read the brief at "),
        "{}",
        s2[0]
    );
    let brief = fs::read_to_string(dir.join("nodes/design/brief.md")).unwrap();
    assert!(brief.contains("## Inputs from upstream nodes"), "{brief}");
    assert!(
        brief.contains("### T survey (id: survey) — status: succeeded"),
        "{brief}"
    );
    assert!(brief.contains("you said: survey the field"), "{brief}");
    assert!(brief.contains("untrusted data"), "{brief}");
    // The side-turn notify came before the deliverable: the node waited for its own.
    assert!(dir.join("nodes/design/deliverables/design.md").is_file());
    let hooks =
        fs::read_to_string(dir.join("runs").join(&run_id).join("sessions/s2/hooks.log")).unwrap();
    assert_eq!(
        hooks.lines().filter(|l| l.starts_with("stop")).count(),
        2,
        "{hooks}"
    );
    assert!(hooks.lines().all(|l| l.starts_with("stop {")), "{hooks}");
    let first = hooks.lines().next().unwrap();
    assert!(
        first.contains("Generate a concise title"),
        "side turn first: {hooks}"
    );
    // The report node follows a codex session: brief form too.
    let s3 = received(dir, &run_id, "s3");
    assert!(
        s3[0].starts_with("Node \"T report\" (report): read the brief"),
        "{s3:?}"
    );
    assert_eq!(received(dir, &run_id, "s1"), ["survey the field"]);
    for id in ["survey", "design", "report"] {
        let reply = dir.join("runs").join(&run_id).join(id).join("reply.md");
        assert!(reply.is_file(), "{id}: no reply.md");
    }
    let design_reply =
        fs::read_to_string(dir.join("runs").join(&run_id).join("design/reply.md")).unwrap();
    assert!(
        design_reply.contains("done (1 file(s) written)"),
        "{design_reply}"
    );
    // The node's Claude model is not typed into Codex.
    assert!(!s2.iter().any(|l| l.starts_with("/model")));
    kill_sessions(&app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_custom_harness_with_quiet_stop_runs_a_node() {
    use_fakes();
    let g = graph(
        vec![node("solo", 0.0, 0.0, json!({ "prompt": "solo task" }))],
        &[],
        json!({ "max_parallel": 1, "default_permission_mode": "claude_settings",
                "harness": "fakecli" }),
    );
    let tmp = graph_dir(&g);
    let dir = tmp.path();
    let app = app_for(dir);
    let profile = HarnessProfile {
        id: "fakecli".into(),
        label: "Fake CLI".into(),
        kind: HarnessKind::Custom,
        command: Some(node_path()),
        args: vec![script("fake-codex.mjs"), "--fake-no-notify".into()],
        env: [("FAKE_CUSTOM".to_string(), "1".to_string())]
            .into_iter()
            .collect(),
        model: None,
        builtin: false,
        idle_marker: Some("Ask Codex to do anything".into()),
        dialog_markers: vec!["Trust this folder".into()],
        stop_mode: StopMode::Quiet,
        quiet_ms: Some(1500),
    };
    let list = app.save_harness(&profile).unwrap();
    assert_eq!(list.last().unwrap().id, "fakecli");
    let rec = Arc::new(Recorder::default());
    let (run_id, state) = run(&app, dir, RunTarget::All, &rec).await;
    assert_eq!(state.status, RunStatus::Succeeded, "{state:#?}");
    assert_eq!(state.sessions[0].harness, "fakecli");
    assert_eq!(state.sessions[0].title, "T solo (Fake CLI)");
    let got = fs::read_to_string(dir.join("fake-received.log")).unwrap();
    assert_eq!(got.lines().collect::<Vec<_>>(), ["solo task"]);
    let reply = fs::read_to_string(dir.join("runs").join(&run_id).join("solo/reply.md")).unwrap();
    assert!(reply.contains("done (0 file(s) written)"), "{reply}");
    kill_sessions(&app, &run_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_env_variable_fails_the_session_before_anything_spawns() {
    use_fakes();
    let g = graph(
        vec![node(
            "a",
            0.0,
            0.0,
            json!({ "prompt": "hello", "harness": "needsvar" }),
        )],
        &[],
        settings(1, false),
    );
    let tmp = graph_dir(&g);
    let dir = tmp.path();
    let app = app_for(dir);
    // Not defined yet: the run does not start.
    let rec = Arc::new(Recorder::default());
    let err = app
        .start_by_mode(dir, RunTarget::All, rec.clone())
        .expect_err("undefined harness refused");
    assert_eq!(err.code, "invalid_input");
    assert_eq!(
        err.message,
        "node a: harness needsvar is not defined on this machine; add it under Settings \u{2192} Harnesses"
    );
    let profile = HarnessProfile {
        id: "needsvar".into(),
        label: "Needs a variable".into(),
        kind: HarnessKind::Claude,
        command: None,
        args: Vec::new(),
        env: [(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            "${MISSING_VAR}".to_string(),
        )]
        .into_iter()
        .collect(),
        model: None,
        builtin: false,
        idle_marker: None,
        dialog_markers: Vec::new(),
        stop_mode: StopMode::Quiet,
        quiet_ms: None,
    };
    std::env::remove_var("MISSING_VAR");
    app.save_harness(&profile).unwrap();
    let (run_id, state) = run(&app, dir, RunTarget::All, &rec).await;
    assert_eq!(state.status, RunStatus::Failed);
    let a = &state.nodes["a"];
    assert_eq!(a.status, NodeStatus::Failed);
    let reason = a.reason.clone().unwrap();
    assert!(
        reason.contains("environment variable MISSING_VAR")
            && reason.contains("is not set")
            && reason.contains("harness needsvar"),
        "{reason}"
    );
    assert_eq!(state.sessions[0].status, SessionStatus::Failed);
    assert_eq!(state.sessions[0].pid, None);
    let sdir = dir.join("runs").join(&run_id).join("sessions/s1");
    assert!(
        !sdir.join("transcript.log").exists(),
        "no process was spawned"
    );
    assert!(!sdir.join("fake-received.log").exists());
    assert!(rec.windows.lock().unwrap().is_empty(), "no window either");
}

#[test]
fn the_notify_helper_appends_stop_whatever_the_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("hooks.log");
    let hostile = "\"; del /q * & echo pwned | more\r\n%PATH% $(rm -rf /) `x` \u{0}";
    let hostile = hostile.replace('\u{0}', "");
    let json = r#"{"type":"agent-turn-complete","input-messages":["a \"b\" & c | d\ne"]}"#;
    for payload in [hostile.as_str(), json] {
        let status = std::process::Command::new(APP_EXE)
            .arg("--notify")
            .arg(&log)
            .arg(payload)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let status = std::process::Command::new(APP_EXE)
        .args(["--notify"])
        .arg(&log)
        .status()
        .unwrap();
    assert!(status.success());
    let text = fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert_eq!(lines[0], "stop");
    assert!(lines[1].starts_with("stop {") && lines[1].contains(r#"a \"b\" & c | d\ne"#));
    assert_eq!(lines[2], "stop");
    // Only a hooks.log is written.
    let other = tmp.path().join("other.txt");
    let status = std::process::Command::new(APP_EXE)
        .arg("--notify")
        .arg(&other)
        .arg("{}")
        .status()
        .unwrap();
    assert!(!status.success());
    assert!(!other.exists());
}

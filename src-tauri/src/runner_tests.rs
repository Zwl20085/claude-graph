use std::sync::Mutex;

use super::*;
use crate::contract::Graph;
use crate::prompt::INSTRUCTION;

fn example() -> Graph {
    serde_json::from_str(include_str!("../../examples/hello-pipeline/graph.json")).unwrap()
}

fn ok_result() -> ResultInfo {
    ResultInfo {
        subtype: "success".into(),
        is_error: false,
        result: Some("x".into()),
        ..Default::default()
    }
}

#[test]
fn args_for_example_nodes() {
    let g = example();
    let dir = Path::new("/work/g");
    let design = &g.nodes[1];
    assert_eq!(
        build_args(design, &g.settings, dir, None, INSTRUCTION),
        vec![
            "-p",
            INSTRUCTION,
            "--output-format",
            "stream-json",
            "--verbose",
            "--model",
            "opus",
            "--permission-mode",
            "acceptEdits",
            "--add-dir",
            &dir.display().to_string(),
            "--max-turns",
            "60",
            "--allowedTools",
            "Read",
            "Write",
            "Edit",
            "Bash(python:*)",
        ]
    );
    // report: model sonnet, permission inherits acceptEdits, no tools/turns; resumed.
    let report = &g.nodes[2];
    let args = build_args(report, &g.settings, dir, Some("sess-1"), INSTRUCTION);
    assert!(args
        .windows(2)
        .any(|w| w == ["--permission-mode", "acceptEdits"]));
    assert!(args.windows(2).any(|w| w == ["--resume", "sess-1"]));
    assert!(!args.contains(&"--allowedTools".to_string()));
    assert!(!args.contains(&"--max-turns".to_string()));
}

#[test]
fn inherit_everywhere_means_no_flag() {
    let mut g = example();
    g.settings.default_model = Model::Inherit;
    g.settings.default_permission_mode = PermissionMode::Inherit;
    let mut node = g.nodes[2].clone();
    node.model = Model::Inherit;
    node.allowed_tools = Some(vec![" ".into()]);
    let args = build_args(&node, &g.settings, Path::new("/g"), None, "i");
    assert!(!args.contains(&"--model".to_string()));
    assert!(!args.contains(&"--permission-mode".to_string()));
    assert!(!args.contains(&"--allowedTools".to_string()));
    assert_eq!(
        permission_flag(PermissionMode::DontAsk, PermissionMode::Plan),
        Some("dontAsk")
    );
}

#[test]
fn status_line_detection() {
    assert_eq!(
        final_status("x\n## Status: DONE"),
        Some((true, String::new()))
    );
    assert_eq!(
        final_status("## Status: BLOCKED\nNo data.\nReally."),
        Some((false, "No data. Really.".into()))
    );
    assert_eq!(
        final_status("## Status: BLOCKED\nfirst\n## Status: DONE\n"),
        Some((true, String::new()))
    );
    assert_eq!(final_status("no status"), None);
}

#[test]
fn evaluation_rules() {
    let done = "## Status: DONE";
    let ok = ok_result();
    assert_eq!(
        evaluate(Some(0), Some(&ok), Some(done), &[], ""),
        (NodeStatus::Succeeded, None)
    );

    let (s, r) = evaluate(Some(0), Some(&ok), Some("## Status: BLOCKED\nwhy"), &[], "");
    assert_eq!(s, NodeStatus::Failed);
    assert!(r.unwrap().contains("BLOCKED: why"));

    let (_, r) = evaluate(Some(0), Some(&ok), Some("hello"), &[], "");
    assert!(r.unwrap().contains("no '## Status: DONE'"));

    let (_, r) = evaluate(Some(0), Some(&ok), Some(done), &["/x/a.md".into()], "");
    assert_eq!(r.unwrap(), "missing deliverables: /x/a.md");

    let (_, r) = evaluate(Some(1), Some(&ok), Some(done), &[], "");
    assert_eq!(r.unwrap(), "claude exited with code 1");

    let (_, r) = evaluate(Some(143), None, None, &[], "auth failed");
    assert_eq!(
        r.unwrap(),
        "claude exited with code 143 without a result: auth failed"
    );

    let err = ResultInfo {
        subtype: "error_max_turns".into(),
        is_error: true,
        errors: vec!["Reached max turns".into()],
        ..Default::default()
    };
    let (_, r) = evaluate(Some(1), Some(&err), None, &[], "");
    assert_eq!(
        r.unwrap(),
        "claude reported error_max_turns: Reached max turns"
    );
}

#[derive(Default)]
struct Recorder(Mutex<Vec<NodeEvent>>);

impl Emitter for Recorder {
    fn run_state(&self, _: &crate::contract::RunState) {}
    fn node_event(&self, e: &NodeEvent) {
        self.0.lock().unwrap().push(e.clone());
    }
}

fn fake_claude() -> ClaudeCommand {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/fake-claude.mjs");
    let env = crate::claude_bin::ResolveEnv {
        configured: None,
        env_override: Some(format!("node {}", script.display())),
        ..crate::claude_bin::ResolveEnv::from_process(None)
    };
    match crate::claude_bin::resolve(&env) {
        crate::claude_bin::Resolution::Found(c) => c,
        other => panic!("node not found: {other:?}"),
    }
}

/// Spawns the fake CLI; a cancel mid-stream kills it and reports `cancelled`.
#[tokio::test]
async fn kill_flag_cancels_a_running_turn() {
    let tmp = tempfile::tempdir().unwrap();
    // A brief with no deliverables; the delay keeps the fake running for a while.
    let spec = TurnSpec {
        run_id: "R".into(),
        node_id: "n".into(),
        claude: ClaudeCommand {
            prefix_args: fake_claude()
                .prefix_args
                .into_iter()
                .chain(["--fake-delay-ms".into(), "400".into()])
                .collect(),
            ..fake_claude()
        },
        args: vec!["-p".into(), "x".into()],
        stdin_text: "no deliverables here".into(),
        cwd: tmp.path().to_path_buf(),
        out_dir: tmp.path().join("out"),
        required: Vec::new(),
    };
    let recorder = Arc::new(Recorder::default());
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(run_turn(spec, recorder.clone(), rx));
    // Wait for the first event, then cancel.
    for _ in 0..200 {
        if !recorder.0.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tx.send(true).unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("cancel must not hang")
        .unwrap();
    assert_eq!(outcome.status, NodeStatus::Cancelled);
    assert!(tmp.path().join("out").join(EVENTS_FILE).is_file());
}

#[tokio::test]
async fn missing_working_dir_fails_without_spawning() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = TurnSpec {
        run_id: "R".into(),
        node_id: "n".into(),
        claude: fake_claude(),
        args: Vec::new(),
        stdin_text: String::new(),
        cwd: tmp.path().join("nope"),
        out_dir: tmp.path().join("out"),
        required: Vec::new(),
    };
    let (_tx, rx) = watch::channel(false);
    let outcome = run_turn(spec, Arc::new(Recorder::default()), rx).await;
    assert_eq!(outcome.status, NodeStatus::Failed);
    assert!(outcome.reason.unwrap().contains("does not exist"));
}

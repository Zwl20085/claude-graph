use std::sync::Mutex;
use std::time::SystemTime;

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
    let resume = Resume {
        session_id: "sess-1",
        fork: false,
    };
    let args = build_args(report, &g.settings, dir, Some(resume), INSTRUCTION);
    assert!(args
        .windows(2)
        .any(|w| w == ["--permission-mode", "acceptEdits"]));
    assert!(args.windows(2).any(|w| w == ["--resume", "sess-1"]));
    assert!(!args.contains(&"--fork-session".to_string()));
    let forked = Resume {
        session_id: "sess-1",
        fork: true,
    };
    let args = build_args(report, &g.settings, dir, Some(forked), INSTRUCTION);
    assert!(args
        .windows(3)
        .any(|w| w == ["--resume", "sess-1", "--fork-session"]));
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
    let none = Deliverables::default();
    assert_eq!(
        evaluate(Some(0), Some(&ok), Some(done), &none, ""),
        (NodeStatus::Succeeded, None)
    );

    let (s, r) = evaluate(
        Some(0),
        Some(&ok),
        Some("## Status: BLOCKED\nwhy"),
        &none,
        "",
    );
    assert_eq!(s, NodeStatus::Failed);
    assert!(r.unwrap().contains("BLOCKED: why"));

    let (_, r) = evaluate(Some(0), Some(&ok), Some("hello"), &none, "");
    assert!(r.unwrap().contains("no '## Status: DONE'"));

    let missing = ["/x/a.md".to_string()];
    let d = Deliverables {
        missing: &missing,
        stale: &[],
    };
    let (_, r) = evaluate(Some(0), Some(&ok), Some(done), &d, "");
    assert_eq!(r.unwrap(), "missing deliverables: /x/a.md");

    let stale = ["/x/old.md".to_string()];
    let d = Deliverables {
        missing: &[],
        stale: &stale,
    };
    let (s, r) = evaluate(Some(0), Some(&ok), Some(done), &d, "");
    assert_eq!(s, NodeStatus::Failed);
    assert_eq!(
        r.unwrap(),
        "deliverable /x/old.md was not written in this run"
    );

    let (_, r) = evaluate(Some(1), Some(&ok), Some(done), &none, "");
    assert_eq!(r.unwrap(), "claude exited with code 1");

    let (_, r) = evaluate(Some(143), None, None, &none, "auth failed");
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
    let (_, r) = evaluate(Some(1), Some(&err), None, &none, "");
    assert_eq!(
        r.unwrap(),
        "claude reported error_max_turns: Reached max turns"
    );
}

#[test]
fn deliverable_freshness() {
    let tmp = tempfile::tempdir().unwrap();
    let fresh = tmp.path().join("fresh.md");
    let old = tmp.path().join("old.md");
    let gone = tmp.path().join("gone.md");
    std::fs::write(&fresh, "x").unwrap();
    std::fs::write(&old, "x").unwrap();
    let hour_ago = SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();
    // Written one second before the turn "started": within the 2 s slack.
    let since = SystemTime::now() + Duration::from_secs(1);
    let (missing, stale) = check_deliverables(&[fresh, old.clone(), gone.clone()], since);
    assert_eq!(missing, vec![gone.display().to_string()]);
    assert_eq!(stale, vec![old.display().to_string()]);
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

fn spec_with(claude: ClaudeCommand, dir: &Path, stdin: &str, required: Vec<PathBuf>) -> TurnSpec {
    TurnSpec {
        run_id: "R".into(),
        node_id: "n".into(),
        claude,
        args: vec!["-p".into(), "x".into()],
        stdin_text: stdin.into(),
        cwd: dir.to_path_buf(),
        out_dir: dir.join("out"),
        required,
    }
}

fn with_extra(extra: &[&str]) -> ClaudeCommand {
    let base = fake_claude();
    ClaudeCommand {
        prefix_args: base
            .prefix_args
            .iter()
            .cloned()
            .chain(extra.iter().map(|a| a.into()))
            .collect(),
        ..base
    }
}

/// A deliverable left over from an earlier run does not satisfy this turn.
#[tokio::test]
async fn stale_deliverable_fails_the_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join("old.md");
    std::fs::write(&old, "from an earlier run").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    let spec = spec_with(
        fake_claude(),
        tmp.path(),
        "no deliverables listed",
        vec![old],
    );
    let (_tx, rx) = watch::channel(false);
    let outcome = run_turn(spec, Arc::new(Recorder::default()), rx).await;
    assert_eq!(outcome.status, NodeStatus::Failed);
    assert!(
        outcome
            .reason
            .as_deref()
            .unwrap()
            .contains("was not written in this run"),
        "{outcome:?}"
    );
}

/// A grandchild holding stdout open after the CLI exited must not lose the
/// parsed result, and must not make the turn wait for it.
#[tokio::test(flavor = "multi_thread")]
async fn pipe_held_by_grandchild_keeps_result_and_does_not_block() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = spec_with(
        with_extra(&["--hold-stdout-ms", "30000"]),
        tmp.path(),
        "no deliverables here",
        Vec::new(),
    );
    let (_tx, rx) = watch::channel(false);
    let t0 = Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(25),
        run_turn(spec, Arc::new(Recorder::default()), rx),
    )
    .await
    .expect("turn must not wait for the held pipe");
    let took = t0.elapsed();
    assert_eq!(outcome.status, NodeStatus::Succeeded, "{outcome:?}");
    assert_eq!(outcome.cost_usd, Some(0.0123));
    assert!(outcome.session_id.is_some());
    assert!(took < Duration::from_secs(12), "took {took:?}");
}

/// A stdout line over LINE_CAP is truncated and reported, the rest still parses.
#[tokio::test(flavor = "multi_thread")]
async fn overlong_stdout_line_is_truncated() {
    let tmp = tempfile::tempdir().unwrap();
    let script = "process.stdout.write('x'.repeat(1536 * 1024) + '\\n'); \
        process.stdout.write(JSON.stringify({type:'result',subtype:'success',is_error:false,\
        result:'## Status: DONE',session_id:'s'}) + '\\n');";
    let node = fake_claude().program;
    let claude = ClaudeCommand {
        program: node,
        prefix_args: vec!["-e".into(), script.into()],
        display: "node -e".into(),
    };
    let mut spec = spec_with(claude, tmp.path(), "", Vec::new());
    spec.args = Vec::new();
    let recorder = Arc::new(Recorder::default());
    let (_tx, rx) = watch::channel(false);
    let outcome = run_turn(spec, recorder.clone(), rx).await;
    assert_eq!(outcome.status, NodeStatus::Succeeded, "{outcome:?}");
    let events = recorder.0.lock().unwrap().clone();
    assert!(events
        .iter()
        .any(|e| e.kind == NodeEventKind::ParseError && e.text.contains("truncated")));
    let log = std::fs::read_to_string(tmp.path().join("out").join(EVENTS_FILE)).unwrap();
    assert!(log.lines().all(|l| l.len() <= LINE_CAP));
}

/// Windows: an npm-style `claude.cmd` in a folder with a space, run against a
/// graph folder whose name holds `&` and a space, is spawned without `cmd /C`.
/// It must either work or fail cleanly with `spawn_failed`, never run the text
/// after `&` as a command.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn batch_shim_with_hostile_paths_never_executes_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let base = crate::paths::abs(tmp.path());
    let bin = base.join("npm bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = fake_claude();
    let node = fake.program.clone();
    let script = PathBuf::from(&fake.prefix_args[0]);
    let shim = bin.join("claude.cmd");
    std::fs::write(
        &shim,
        format!("@\"{}\" \"{}\" %*\r\n", node.display(), script.display()),
    )
    .unwrap();
    let graph = base.join("g & mkdir INJECTED");
    std::fs::create_dir_all(&graph).unwrap();
    let env = crate::claude_bin::ResolveEnv::from_process(None);
    let claude = ClaudeCommand::for_path(shim.clone(), &env);
    assert_eq!(
        claude.program, shim,
        "no cli.js next to it: the shim itself"
    );

    let g = example();
    let survey = g.nodes[0].clone();
    let mut hostile_node = survey.clone();
    hostile_node.allowed_tools = None;
    let cases = [
        build_args(&hostile_node, &g.settings, &graph, None, INSTRUCTION),
        vec![
            "-p".to_string(),
            "x\" & mkdir INJECTED2 & \"".to_string(),
            "--add-dir".to_string(),
            "%CD% & mkdir INJECTED3".to_string(),
        ],
    ];
    for args in cases {
        let spec = TurnSpec {
            run_id: "R".into(),
            node_id: "n".into(),
            claude: claude.clone(),
            args,
            stdin_text: "no deliverables".into(),
            cwd: graph.clone(),
            out_dir: graph.join("out"),
            required: Vec::new(),
        };
        println!("spawn: {}", display_command(&spec));
        let (_tx, rx) = watch::channel(false);
        let outcome = run_turn(spec, Arc::new(Recorder::default()), rx).await;
        println!("-> {:?} {:?}", outcome.status, outcome.reason);
        let clean_failure = outcome
            .reason
            .as_deref()
            .is_some_and(|r| r.starts_with("spawn_failed"));
        assert!(
            outcome.status == NodeStatus::Succeeded || clean_failure,
            "{outcome:?}"
        );
    }
    for dir in [&graph, &base, &bin, &std::env::current_dir().unwrap()] {
        for name in ["INJECTED", "INJECTED2", "INJECTED3"] {
            assert!(!dir.join(name).exists(), "{} in {}", name, dir.display());
        }
    }
}

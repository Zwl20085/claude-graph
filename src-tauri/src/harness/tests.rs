use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::adapter::{behavior, spawn_spec, SpawnInput, StopSignal};
use super::profile::{builtins, CODEX_ID, DEEPSEEK_ID};
use super::*;
use crate::contract::StopMode;
use crate::terminal::deliver::Expect;

fn claude_cmd() -> ClaudeCommand {
    ClaudeCommand {
        program: PathBuf::from("/bin/claude"),
        prefix_args: vec![OsString::from("cli.js")],
        display: "claude".into(),
    }
}

fn lookup(name: &str) -> Option<String> {
    (name == "DEEPSEEK_API_KEY").then(|| "sk-test".into())
}

fn spec(profile: &HarnessProfile, resolve: &ResolveEnv) -> AppResult<adapter::Launch> {
    let claude = claude_cmd();
    spawn_spec(&SpawnInput {
        profile,
        claude: Some(&claude),
        graph_dir: Path::new("/g"),
        cwd: Path::new("/g/work"),
        session_dir: Path::new("/g/runs/R/sessions/s1"),
        notify_exe: Path::new("/app/claude-graph"),
        permission: Some("acceptEdits"),
        lookup: &lookup,
        resolve,
    })
}

fn strs(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|a| a.to_string_lossy().replace('\\', "/"))
        .collect()
}

#[test]
fn claude_profiles_keep_todays_command_and_add_env() {
    let b = builtins();
    let none = ResolveEnv::default();
    let l = spec(&b[0], &none).unwrap();
    assert_eq!(l.program, PathBuf::from("/bin/claude"));
    assert_eq!(
        strs(&l.args),
        [
            "cli.js",
            "--settings",
            "/g/runs/R/sessions/s1/hooks.json",
            "--add-dir",
            "/g",
            "--permission-mode",
            "acceptEdits"
        ]
    );
    assert!(l.env.is_empty());
    let ds = spec(&b[1], &none).unwrap();
    assert_eq!(ds.args, l.args, "deepseek differs only by env");
    assert!(ds
        .env
        .contains(&("ANTHROPIC_AUTH_TOKEN".into(), Some("sk-test".into()))));
    let with_model = HarnessProfile {
        model: Some("opus".into()),
        ..b[0].clone()
    };
    assert!(
        strs(&spec(&with_model, &none).unwrap().args).ends_with(&["--model".into(), "opus".into()])
    );
}

#[test]
fn a_missing_variable_fails_before_any_program_is_resolved() {
    let mut ds = builtins()[1].clone();
    ds.env.insert("X".into(), "${MISSING_VAR}".into());
    let err = spec(&ds, &ResolveEnv::default()).unwrap_err();
    assert!(err.message.contains("MISSING_VAR"), "{}", err.message);
    assert!(!err.message.contains("sk-test"));
}

#[test]
fn codex_args_carry_cwd_model_and_notify() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join(crate::claude_bin::exe_name("codex"));
    std::fs::write(&exe, "").unwrap();
    let resolve = ResolveEnv {
        path_var: Some(tmp.path().as_os_str().to_owned()),
        pathext: Some(".EXE".into()),
        ..Default::default()
    };
    let codex = HarnessProfile {
        model: Some("gpt-5".into()),
        command: Some(exe.display().to_string()),
        ..builtins()[2].clone()
    };
    let l = spec(&codex, &resolve).unwrap();
    assert_eq!(l.program, exe);
    let args = strs(&l.args);
    assert_eq!(&args[..5], ["-C", "/g/work", "-m", "gpt-5", "-c"]);
    assert!(args[5].starts_with("notify=[\"/app/claude-graph\",\"--notify\","));
    assert!(args[5].ends_with("hooks.log\"]"));
    assert_eq!(args.iter().filter(|a| *a == "--no-alt-screen").count(), 1);
    assert_eq!(
        args.last().unwrap(),
        "--no-alt-screen",
        "from the profile args"
    );
    let b = behavior(&codex);
    assert_eq!(b.label, "Codex CLI");
    assert!(!b.start_hook && !b.types_model);
    assert!(matches!(b.expect, Expect::Gone(_)));
    assert_eq!(b.stop, StopSignal::Hook);
}

#[test]
fn custom_args_follow_the_stop_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let prog = tmp.path().join("tool.exe");
    std::fs::write(&prog, "").unwrap();
    let base = HarnessProfile {
        id: "mine".into(),
        label: "Mine".into(),
        kind: HarnessKind::Custom,
        command: Some(prog.display().to_string()),
        args: vec!["--x".into()],
        idle_marker: Some("ready>".into()),
        quiet_ms: Some(1500),
        ..builtins()[0].clone()
    };
    let none = ResolveEnv::default();
    assert_eq!(strs(&spec(&base, &none).unwrap().args), ["--x"]);
    let hooks = HarnessProfile {
        stop_mode: StopMode::ClaudeHooks,
        ..base.clone()
    };
    assert_eq!(strs(&spec(&hooks, &none).unwrap().args)[0], "--settings");
    assert!(behavior(&hooks).start_hook);
    let notify = HarnessProfile {
        stop_mode: StopMode::CodexNotify,
        ..base.clone()
    };
    assert_eq!(strs(&spec(&notify, &none).unwrap().args)[0], "-c");
    assert_eq!(
        behavior(&base).stop,
        StopSignal::Quiet(Duration::from_millis(1500))
    );
    let missing = HarnessProfile {
        command: Some(tmp.path().join("nope.exe").display().to_string()),
        ..base
    };
    assert_eq!(spec(&missing, &none).unwrap_err().code, "not_found");
}

#[test]
fn effective_harness_and_run_start_checks() {
    let g: Graph = serde_json::from_value(json!({
        "version": 1, "name": "g",
        "settings": { "harness": "claude" },
        "nodes": [
            { "id": "a" },
            { "id": "b", "harness": "codex" },
            { "id": "c", "harness": "gone" },
            { "id": "d", "harness": "  " }
        ],
        "edges": []
    }))
    .unwrap();
    assert_eq!(effective(&g.nodes[0], &g.settings), "claude");
    assert_eq!(effective(&g.nodes[1], &g.settings), CODEX_ID);
    assert_eq!(effective(&g.nodes[3], &g.settings), "claude");
    let all = builtins();
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(check_targets(&g, &ids(&["a", "b"]), &all, false).is_ok());
    let err = check_targets(&g, &ids(&["a", "c"]), &all, false).unwrap_err();
    assert_eq!(err.code, "invalid_input");
    assert_eq!(
        err.message,
        "node c: harness gone is not defined on this machine; add it under Settings \u{2192} Harnesses"
    );
    let err = check_targets(&g, &ids(&["b"]), &all, true).unwrap_err();
    assert!(
        err.message.contains("terminal mode only"),
        "{}",
        err.message
    );
    let older: Graph = serde_json::from_value(json!({"version": 1, "name": "g"})).unwrap();
    assert_eq!(older.settings.harness, "claude");
    assert!(find(&all, DEEPSEEK_ID).is_some());
}

#[tokio::test]
async fn check_reports_version_or_message_without_env_values() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/fake-codex.mjs");
    let node = crate::claude_bin::find_on_path(
        "node",
        std::env::var_os("PATH").as_ref(),
        std::env::var("PATHEXT").ok().as_deref(),
    )
    .expect("node on PATH");
    let fake = HarnessProfile {
        id: "fake".into(),
        label: "Fake".into(),
        kind: HarnessKind::Custom,
        command: Some(node.display().to_string()),
        args: vec![script.display().to_string()],
        ..builtins()[0].clone()
    };
    // `<node> --version` comes first, so this checks node itself.
    let ok = check(&fake, None).await;
    assert!(ok.ok, "{ok:?}");
    assert!(ok.version.unwrap().starts_with('v'));
    let mut bad = fake.clone();
    bad.env
        .insert("K".into(), "${CLAUDE_GRAPH_SURELY_MISSING}".into());
    let failed = check(&bad, None).await;
    assert!(!failed.ok);
    assert!(failed
        .message
        .unwrap()
        .contains("CLAUDE_GRAPH_SURELY_MISSING"));
}

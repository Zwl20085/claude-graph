use std::path::PathBuf;

use super::*;
use crate::assistant::charter::CHARTER;
use crate::assistant::journal::charter_file;

const SID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn claude() -> ClaudeCommand {
    ClaudeCommand {
        program: PathBuf::from("claude"),
        prefix_args: Vec::new(),
        display: "claude".into(),
    }
}

fn outcome(status: NodeStatus, text: Option<&str>) -> TurnOutcome {
    TurnOutcome {
        status,
        reason: (status != NodeStatus::Succeeded).then(|| "turn reason".to_string()),
        session_id: Some(SID.into()),
        cost_usd: None,
        num_turns: None,
        duration_ms: 1,
        is_error: false,
        final_text: text.map(str::to_string),
        model: Some("claude-x".into()),
        permission_denials: Vec::new(),
    }
}

fn graph_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let g =
        serde_json::from_str(include_str!("../../../examples/hello-pipeline/graph.json")).unwrap();
    crate::store::save_graph(tmp.path(), &g).unwrap();
    tmp
}

#[test]
fn args_carry_no_node_flags() {
    let dir = Path::new("/work/g");
    let fresh = build_args(dir, None);
    assert_eq!(
        fresh,
        vec![
            "-p",
            INSTRUCTION,
            "--output-format",
            "stream-json",
            "--verbose",
            "--add-dir",
            &dir.display().to_string()
        ]
    );
    let resumed = build_args(dir, Some(SID));
    assert_eq!(&resumed[7..], &["--resume", SID]);
    for flag in [
        "--model",
        "--permission-mode",
        "--allowedTools",
        "--max-turns",
    ] {
        assert!(!resumed.iter().any(|a| a == flag), "{flag}");
    }
}

#[test]
fn prompt_shapes() {
    assert_eq!(
        fresh_prompt("# C\n", "W", "go"),
        "# C\n\n## Workflow\n\nW\n\n## Instruction\n\ngo\n"
    );
    assert_eq!(
        resumed_prompt(None, "yes"),
        "## Message from the assistant\n\nyes\n"
    );
    assert_eq!(
        resumed_prompt(Some("W2"), "yes"),
        "## Workflow (updated)\n\nW2\n\n## Message from the assistant\n\nyes\n"
    );
    assert_eq!(sha256_hex("").len(), 64);
    assert_eq!(
        &sha256_hex("abc")[..8],
        "ba7816bf",
        "standard sha256 test vector"
    );
}

#[test]
fn prepare_fresh_then_resumed() {
    let tmp = graph_dir();
    let dir = tmp.path();
    let now = Utc::now();
    let first = prepare(dir, &AssistantSession::empty(now), "Run it", claude(), now).unwrap();
    assert_eq!(first.turn, 1);
    assert!(!first.resumed);
    assert_eq!(first.message.kind, AssistantKind::Instruction);
    assert_eq!(first.session.status, AssistantStatus::Working);
    assert!(first.spec.stdin_text.starts_with(CHARTER.trim_end()));
    assert!(first
        .spec
        .stdin_text
        .contains("## Workflow\n\nGlobal limit:"));
    assert!(first
        .spec
        .stdin_text
        .ends_with("## Instruction\n\nRun it\n"));
    assert!(charter_file(dir).is_file());
    assert_eq!(first.spec.run_id, "assistant");
    assert_eq!(first.spec.node_id, "scheduler");
    assert_eq!(first.spec.cwd, dir);
    assert!(first.spec.required.is_empty());
    assert!(first.spec.out_dir.join("prompt.md").is_file());

    let waiting = AssistantSession {
        session_id: Some(SID.into()),
        status: AssistantStatus::Waiting,
        ..first.session.clone()
    };
    let second = prepare(dir, &waiting, "Yes", claude(), now).unwrap();
    assert_eq!(second.turn, 2);
    assert!(second.resumed);
    assert_eq!(second.message.kind, AssistantKind::Answer);
    assert_eq!(
        second.spec.stdin_text, "## Message from the assistant\n\nYes\n",
        "unchanged workflow is not resent"
    );
    assert_eq!(&second.spec.args[7..], &["--resume", SID]);

    let stale = AssistantSession {
        workflow_hash: Some("old".into()),
        ..waiting.clone()
    };
    let third = prepare(dir, &stale, "Again", claude(), now).unwrap();
    assert!(third
        .spec
        .stdin_text
        .starts_with("## Workflow (updated)\n\nGlobal limit:"));

    let bad = AssistantSession {
        session_id: Some("--dangerously-skip-permissions".into()),
        ..waiting
    };
    let err = prepare(dir, &bad, "x", claude(), now).err().unwrap();
    assert_eq!(err.code, "invalid_input");
}

#[test]
fn successful_turn_maps_sections_and_status() {
    let before = AssistantSession {
        status: AssistantStatus::Working,
        turns: 1,
        ..AssistantSession::empty(Utc::now())
    };
    let o = outcome(
        NodeStatus::Failed, // the node-mode evaluation never matters here
        Some("## Stage: a\nok\n## Question\nGo on?"),
    );
    let (s, msgs) = evaluate(&before, 1, false, &o, Utc::now());
    assert_eq!(s.status, AssistantStatus::Waiting);
    assert_eq!(s.session_id.as_deref(), Some(SID));
    assert_eq!(s.model.as_deref(), Some("claude-x"));
    assert_eq!(msgs.len(), 2);
    assert!(msgs
        .iter()
        .all(|m| m.role == AssistantRole::Scheduler && m.turn == 1));
    assert_eq!(msgs[0].title.as_deref(), Some("a"));

    let done = outcome(NodeStatus::Failed, Some("## Final\nall good"));
    assert_eq!(
        evaluate(&before, 1, false, &done, Utc::now()).0.status,
        AssistantStatus::Done
    );
}

#[test]
fn errors_cancel_empty_reply_and_denials() {
    let before = AssistantSession {
        session_id: Some("11111111-1111-1111-1111-111111111111".into()),
        ..AssistantSession::empty(Utc::now())
    };
    let cancelled = outcome(NodeStatus::Cancelled, None);
    let (s, msgs) = evaluate(&before, 4, true, &cancelled, Utc::now());
    assert_eq!(s.status, AssistantStatus::Cancelled);
    assert_eq!(msgs[0].kind, AssistantKind::System);
    assert_eq!(msgs[0].text, "Turn 4 cancelled");

    let lost = TurnOutcome {
        is_error: true,
        reason: Some("claude reported error: No conversation found with session ID".into()),
        ..outcome(NodeStatus::Failed, None)
    };
    let (s, msgs) = evaluate(&before, 2, true, &lost, Utc::now());
    assert_eq!(s.status, AssistantStatus::Failed);
    assert!(s.last_error.as_deref().unwrap().ends_with(RESET_HINT));
    assert_eq!(
        s.session_id, before.session_id,
        "failed resume keeps its session"
    );
    assert_eq!(msgs[0].kind, AssistantKind::Error);
    let (s, _) = evaluate(&before, 2, false, &lost, Utc::now());
    assert!(!s.last_error.unwrap().contains(RESET_HINT));

    let empty = outcome(NodeStatus::Succeeded, Some("  "));
    let (s, msgs) = evaluate(&before, 1, false, &empty, Utc::now());
    assert_eq!(s.status, AssistantStatus::Failed);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, AssistantRole::App);

    let denied = TurnOutcome {
        permission_denials: vec!["Write".into(), "Bash".into(), "Write".into()],
        ..outcome(NodeStatus::Succeeded, Some("## Note\nblocked"))
    };
    let (s, msgs) = evaluate(&before, 1, false, &denied, Utc::now());
    assert_eq!(s.status, AssistantStatus::Idle);
    let last = msgs.last().unwrap();
    assert_eq!(last.kind, AssistantKind::Error);
    assert_eq!(
        last.text,
        format!("The scheduler was denied these tools: Write, Bash. {DENIAL_HINT}.")
    );
}

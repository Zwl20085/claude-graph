use super::*;
use serde_json::{json, Value};

const EXAMPLE: &str = include_str!("../../examples/hello-pipeline/graph.json");

/// Positions are f64, so `80` re-serialises as `80.0`; compare numbers by value.
fn numbers_as_f64(v: Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64()),
        Value::Array(a) => Value::Array(a.into_iter().map(numbers_as_f64).collect()),
        Value::Object(o) => {
            Value::Object(o.into_iter().map(|(k, v)| (k, numbers_as_f64(v))).collect())
        }
        other => other,
    }
}

#[test]
fn example_graph_round_trips() {
    let graph: Graph = serde_json::from_str(EXAMPLE).expect("example parses");
    assert_eq!(graph.version, GRAPH_VERSION);
    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.edges.len(), 2);
    let design = graph.nodes.iter().find(|n| n.id == "design").unwrap();
    assert_eq!(design.resources, vec!["cpu".to_string()]);
    assert_eq!(design.model, Model::Opus);
    assert_eq!(design.permission_mode, PermissionMode::AcceptEdits);

    let mut original: Value = serde_json::from_str(EXAMPLE).unwrap();
    // Fields added after the example was written load as defaults and are written back.
    for node in original["nodes"].as_array_mut().unwrap() {
        node.as_object_mut()
            .unwrap()
            .entry("prompt")
            .or_insert(json!(""));
    }
    let reserialised = serde_json::to_value(&graph).unwrap();
    assert_eq!(
        numbers_as_f64(original),
        numbers_as_f64(reserialised.clone()),
        "serialisation must be lossless"
    );
    let again: Graph = serde_json::from_value(reserialised).unwrap();
    assert_eq!(graph, again);
}

#[test]
fn minimal_node_loads_with_defaults() {
    let g: Graph = serde_json::from_value(json!({
        "version": 1,
        "name": "old",
        "nodes": [{ "id": "a" }]
    }))
    .unwrap();
    let n = &g.nodes[0];
    assert_eq!(n.model, Model::Inherit);
    assert_eq!(n.permission_mode, PermissionMode::Inherit);
    assert!(n.allowed_tools.is_none());
    assert!(n.deliverables.is_empty());
    assert_eq!(g.settings, Settings::default());
    assert!(g.edges.is_empty());
}

#[test]
fn run_target_json_shapes() {
    let cases = [
        (json!("all"), RunTarget::All),
        (
            json!({ "from": "survey" }),
            RunTarget::From("survey".into()),
        ),
        (
            json!({ "failed_of": "20261107T101500Z" }),
            RunTarget::FailedOf("20261107T101500Z".into()),
        ),
    ];
    for (value, target) in cases {
        let parsed: RunTarget = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed, target);
        assert_eq!(serde_json::to_value(&target).unwrap(), value);
    }
}

#[test]
fn enum_values_match_brief() {
    assert_eq!(json!(NodeStatus::Cancelled), json!("cancelled"));
    assert_eq!(json!(RunStatus::Running), json!("running"));
    assert_eq!(json!(NodeEventKind::ToolUse), json!("tool_use"));
    assert_eq!(json!(NodeEventKind::ParseError), json!("parse_error"));
    assert_eq!(json!(PermissionMode::AcceptEdits), json!("acceptEdits"));
    assert_eq!(
        json!(PermissionMode::BypassPermissions),
        json!("bypassPermissions")
    );
    assert_eq!(json!(Model::Haiku), json!("haiku"));
    assert_eq!(json!(PermissionMode::DontAsk), json!("dontAsk"));
    assert_eq!(json!(NodeEventKind::UserMessage), json!("user_message"));
    assert_eq!(json!(MessageRole::Assistant), json!("assistant"));
}

#[test]
fn fix_pass_2_names() {
    assert_eq!(
        json!(PermissionMode::ClaudeSettings),
        json!("claude_settings")
    );
    let parsed: PermissionMode = serde_json::from_value(json!("claude_settings")).unwrap();
    assert_eq!(parsed, PermissionMode::ClaudeSettings);
    assert_eq!(json!(NodeStatus::Blocked), json!("blocked"));
    assert_eq!(
        json!(NodeEventKind::PermissionDenied),
        json!("permission_denied")
    );
    let all = [
        NodeStatus::Pending,
        NodeStatus::Queued,
        NodeStatus::Running,
        NodeStatus::Succeeded,
        NodeStatus::Failed,
        NodeStatus::Blocked,
        NodeStatus::Skipped,
        NodeStatus::Cancelled,
    ];
    for s in all {
        assert_eq!(json!(s), json!(s.as_str()));
    }
    assert!(NodeStatus::Blocked.is_failure() && NodeStatus::Failed.is_failure());
    assert!(!NodeStatus::Skipped.is_failure());
    assert!(NodeStatus::Blocked.is_terminal());
    assert!(!NodeStatus::Running.is_terminal());
    let v = json!(NodeRunState::pending());
    assert_eq!(v["permission_denials"], json!([]));
    let counts = json!(StatusCounts::default());
    assert_eq!(counts["blocked"], json!(0));
}

#[test]
fn run_state_round_trips() {
    let mut nodes = BTreeMap::new();
    nodes.insert("survey".to_string(), NodeRunState::pending());
    let state = RunState {
        run_id: "20261107T101500Z".into(),
        graph_dir: "/tmp/g".into(),
        started_at: "2026-11-07T10:15:00Z".parse().unwrap(),
        finished_at: None,
        status: RunStatus::Running,
        nodes,
    };
    let v = serde_json::to_value(&state).unwrap();
    assert_eq!(v["nodes"]["survey"]["status"], json!("pending"));
    assert_eq!(v["started_at"], json!("2026-11-07T10:15:00Z"));
    assert_eq!(v["finished_at"], Value::Null);
    let back: RunState = serde_json::from_value(v).unwrap();
    assert_eq!(back, state);
}

#[test]
fn assistant_names_are_snake_case() {
    assert_eq!(json!(AssistantStatus::Idle), json!("idle"));
    assert_eq!(json!(AssistantStatus::Working), json!("working"));
    assert_eq!(json!(AssistantStatus::Waiting), json!("waiting"));
    assert_eq!(json!(AssistantStatus::Done), json!("done"));
    assert_eq!(json!(AssistantStatus::Failed), json!("failed"));
    assert_eq!(json!(AssistantStatus::Cancelled), json!("cancelled"));
    assert_eq!(json!(AssistantRole::User), json!("user"));
    assert_eq!(json!(AssistantRole::Scheduler), json!("scheduler"));
    assert_eq!(json!(AssistantRole::App), json!("app"));
    let kinds = [
        (AssistantKind::Instruction, "instruction"),
        (AssistantKind::Answer, "answer"),
        (AssistantKind::Stage, "stage"),
        (AssistantKind::Question, "question"),
        (AssistantKind::Decision, "decision"),
        (AssistantKind::Note, "note"),
        (AssistantKind::Final, "final"),
        (AssistantKind::System, "system"),
        (AssistantKind::Error, "error"),
    ];
    for (kind, name) in kinds {
        assert_eq!(json!(kind), json!(name));
    }
    assert_eq!(EVENT_ASSISTANT_UPDATE, "assistant_update");
}

#[test]
fn assistant_state_round_trips_with_contract_field_names() {
    let state = AssistantState {
        session: AssistantSession {
            session_id: Some("0f8fad5b-d9cb-469f-a165-70867728950e".into()),
            status: AssistantStatus::Waiting,
            turns: 2,
            model: Some("claude-sonnet".into()),
            workflow_hash: Some("ab".into()),
            last_error: None,
            updated_at: "2026-11-07T10:15:00Z".parse().unwrap(),
        },
        messages: vec![AssistantMessage {
            id: "m1".into(),
            ts: "2026-11-07T10:15:01Z".parse().unwrap(),
            turn: 1,
            role: AssistantRole::Scheduler,
            kind: AssistantKind::Stage,
            title: Some("survey".into()),
            text: "done".into(),
        }],
    };
    let v = serde_json::to_value(&state).unwrap();
    assert_eq!(
        v,
        json!({
            "session": {
                "session_id": "0f8fad5b-d9cb-469f-a165-70867728950e",
                "status": "waiting",
                "turns": 2,
                "model": "claude-sonnet",
                "workflow_hash": "ab",
                "last_error": null,
                "updated_at": "2026-11-07T10:15:00Z"
            },
            "messages": [{
                "id": "m1",
                "ts": "2026-11-07T10:15:01Z",
                "turn": 1,
                "role": "scheduler",
                "kind": "stage",
                "title": "survey",
                "text": "done"
            }]
        })
    );
    let back: AssistantState = serde_json::from_value(v).unwrap();
    assert_eq!(back, state);
    let empty = AssistantSession::empty(Utc::now());
    assert_eq!(empty.status, AssistantStatus::Idle);
    assert_eq!(empty.turns, 0);
}

#[test]
fn app_error_serialises_as_code_and_message() {
    let e = AppError::not_implemented("open_graph");
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        json!({ "code": "not_implemented", "message": "open_graph is not implemented yet" })
    );
}

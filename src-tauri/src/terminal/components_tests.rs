use super::*;
use crate::contract::{NodeRunState, RunMode, RunStatus};
use serde_json::json;

/// Nodes `(id, x, y)` and edges `(from, to)`.
fn graph(nodes: &[(&str, f64, f64)], edges: &[(&str, &str)]) -> Graph {
    serde_json::from_value(json!({
        "version": 1,
        "name": "g",
        "nodes": nodes.iter().map(|(id, x, y)| json!({
            "id": id, "title": format!("T {id}"), "position": {"x": x, "y": y}
        })).collect::<Vec<_>>(),
        "edges": edges.iter().enumerate().map(|(i, (f, t))| json!({
            "id": format!("e{i}"), "from": f, "to": t
        })).collect::<Vec<_>>(),
    }))
    .unwrap()
}

fn sample() -> Graph {
    // Component A: a -> c, b -> c (two roots); component B: x -> y, above A.
    graph(
        &[
            ("a", 200.0, 100.0),
            ("b", 0.0, 100.0),
            ("c", 0.0, 300.0),
            ("x", 500.0, 0.0),
            ("y", 500.0, 200.0),
            ("z", 900.0, 50.0),
        ],
        &[("a", "c"), ("b", "c"), ("x", "y")],
    )
}

#[test]
fn components_are_ordered_by_position_and_topologically() {
    let comps = components(&sample());
    assert_eq!(
        comps,
        vec![
            vec!["x".to_string(), "y".into()],
            vec!["z".into()],
            vec!["b".into(), "a".into(), "c".into()],
        ]
    );
}

#[test]
fn plan_all_numbers_sessions_and_titles_roots() {
    let plans = plan(&sample(), &RunTarget::All, &[], &Labels::new()).unwrap();
    let sids: Vec<&str> = plans.iter().map(|p| p.sid.as_str()).collect();
    assert_eq!(sids, ["s1", "s2", "s3"]);
    assert_eq!(plans[2].title, "T b + T a");
    assert_eq!(plans[2].to_send(), ["b", "a", "c"]);
    assert!(plans[2].before_start().is_empty());
}

#[test]
fn plan_from_node_keeps_one_component_and_skips_earlier_nodes() {
    let plans = plan(&sample(), &RunTarget::From("a".into()), &[], &Labels::new()).unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].sid, "s3");
    assert_eq!(plans[0].before_start(), ["b"]);
    assert_eq!(plans[0].to_send(), ["a", "c"]);
    let err = plan(
        &sample(),
        &RunTarget::From("nope".into()),
        &[],
        &Labels::new(),
    )
    .unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[test]
fn plan_failed_of_starts_at_the_first_failed_node_per_component() {
    let status = |s| NodeRunState {
        status: s,
        ..NodeRunState::pending()
    };
    let run = RunState {
        run_id: "R1".into(),
        graph_dir: "/g".into(),
        started_at: "2026-11-07T10:00:00Z".parse().unwrap(),
        finished_at: None,
        status: RunStatus::Failed,
        nodes: [
            ("x".to_string(), status(NodeStatus::Succeeded)),
            ("y".to_string(), status(NodeStatus::Succeeded)),
            ("b".to_string(), status(NodeStatus::Succeeded)),
            ("a".to_string(), status(NodeStatus::Failed)),
            ("c".to_string(), status(NodeStatus::Skipped)),
        ]
        .into_iter()
        .collect(),
        mode: RunMode::Terminal,
        sessions: Vec::new(),
    };
    let plans = plan(
        &sample(),
        &RunTarget::FailedOf("R1".into()),
        std::slice::from_ref(&run),
        &Labels::new(),
    )
    .unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].sid, "s3");
    assert_eq!(plans[0].to_send(), ["a", "c"]);
    let err = plan(
        &sample(),
        &RunTarget::FailedOf("R2".into()),
        &[run],
        &Labels::new(),
    )
    .unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[test]
fn long_titles_are_cut() {
    let mut g = graph(&[("a", 0.0, 0.0), ("b", 1.0, 0.0)], &[]);
    g.nodes[0].title = "x".repeat(100);
    let plans = plan(&g, &RunTarget::All, &[], &Labels::new()).unwrap();
    assert_eq!(plans[0].title.chars().count(), TITLE_MAX);
    assert!(plans[0].title.ends_with('…'));
    assert_eq!(plans[1].title, "T b");
}

/// survey(claude) -> design(codex) -> report(claude), and a lone node.
fn mixed() -> Graph {
    let mut g = graph(
        &[
            ("survey", 0.0, 0.0),
            ("design", 0.0, 100.0),
            ("report", 0.0, 200.0),
            ("lone", 500.0, 300.0),
        ],
        &[("survey", "design"), ("design", "report")],
    );
    g.nodes[1].harness = Some("codex".into());
    g.nodes[3].harness = Some("mine".into());
    g
}

fn labels() -> Labels {
    [("codex", "Codex CLI"), ("claude", "Claude Code")]
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .into_iter()
        .collect()
}

#[test]
fn a_chain_is_split_where_the_harness_changes() {
    let plans = plan(&mixed(), &RunTarget::All, &[], &labels()).unwrap();
    let got: Vec<(&str, &str, &str, usize)> = plans
        .iter()
        .map(|p| {
            (
                p.sid.as_str(),
                p.title.as_str(),
                p.harness.as_str(),
                p.component,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("s1", "T survey", "claude", 0),
            ("s2", "T design (Codex CLI)", "codex", 0),
            ("s3", "T report", "claude", 0),
            ("s4", "T lone (mine)", "mine", 1),
        ]
    );
    assert_eq!(plans[1].node_ids, ["design"]);
}

#[test]
fn from_and_failed_of_keep_earlier_sessions_of_the_component_as_skipped() {
    let plans = plan(&mixed(), &RunTarget::From("design".into()), &[], &labels()).unwrap();
    let sends: Vec<(&str, Vec<String>, Vec<String>)> = plans
        .iter()
        .map(|p| {
            (
                p.sid.as_str(),
                p.before_start().to_vec(),
                p.to_send().to_vec(),
            )
        })
        .collect();
    assert_eq!(
        sends,
        [
            ("s1", vec!["survey".to_string()], vec![]),
            ("s2", vec![], vec!["design".to_string()]),
            ("s3", vec![], vec!["report".to_string()]),
        ]
    );
    let status = |s| NodeRunState {
        status: s,
        ..NodeRunState::pending()
    };
    let run = RunState {
        run_id: "R1".into(),
        graph_dir: "/g".into(),
        started_at: "2026-11-07T10:00:00Z".parse().unwrap(),
        finished_at: None,
        status: RunStatus::Failed,
        nodes: [
            ("survey".to_string(), status(NodeStatus::Succeeded)),
            ("design".to_string(), status(NodeStatus::Succeeded)),
            ("report".to_string(), status(NodeStatus::Failed)),
            ("lone".to_string(), status(NodeStatus::Succeeded)),
        ]
        .into_iter()
        .collect(),
        mode: RunMode::Terminal,
        sessions: Vec::new(),
    };
    let plans = plan(
        &mixed(),
        &RunTarget::FailedOf("R1".into()),
        &[run],
        &labels(),
    )
    .unwrap();
    let to_send: Vec<&[String]> = plans.iter().map(|p| p.to_send()).collect();
    assert_eq!(to_send.len(), 3);
    assert!(to_send[0].is_empty() && to_send[1].is_empty());
    assert_eq!(to_send[2], ["report"]);
}

#[test]
fn titles_leave_room_for_the_harness_label() {
    let mut g = mixed();
    g.nodes[1].title = "y".repeat(100);
    let plans = plan(&g, &RunTarget::All, &[], &labels()).unwrap();
    assert_eq!(plans[1].title.chars().count(), TITLE_MAX);
    assert!(
        plans[1].title.ends_with("\u{2026} (Codex CLI)"),
        "{}",
        plans[1].title
    );
}

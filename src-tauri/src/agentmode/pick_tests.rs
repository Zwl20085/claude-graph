use super::*;
use serde_json::json;

/// a -> b, a -> c, b -> d, c -> d; extra free nodes e and f.
fn graph(settings: serde_json::Value, tags: &[(&str, &[&str])]) -> Graph {
    let ids = ["a", "b", "c", "d", "e", "f"];
    let nodes: Vec<_> = ids
        .iter()
        .map(|id| {
            let res: Vec<&str> = tags
                .iter()
                .find(|(n, _)| n == id)
                .map(|(_, t)| t.to_vec())
                .unwrap_or_default();
            json!({ "id": id, "title": id.to_uppercase(), "resources": res })
        })
        .collect();
    serde_json::from_value(json!({
        "version": 1, "name": "g", "settings": settings, "nodes": nodes,
        "edges": [
            {"id": "e1", "from": "a", "to": "b"}, {"id": "e2", "from": "a", "to": "c"},
            {"id": "e3", "from": "b", "to": "d"}, {"id": "e4", "from": "c", "to": "d"}
        ]
    }))
    .unwrap()
}

fn st(pairs: &[(&str, NodeStatus)]) -> Statuses {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn all_pending() -> Statuses {
    ["a", "b", "c", "d", "e", "f"]
        .iter()
        .map(|k| (k.to_string(), NodeStatus::Pending))
        .collect()
}

#[test]
fn dependencies_gate_and_priority_orders() {
    let g = graph(json!({"max_parallel": 4}), &[]);
    // a has the longest downstream path; e and f are sinks (id order).
    assert_eq!(ready(&g, &all_pending()), ["a", "e", "f"]);
    assert_eq!(pick(&g, &all_pending(), 1, None), ["a"]);
    let mut s = all_pending();
    s.insert("a".into(), NodeStatus::Succeeded);
    assert_eq!(ready(&g, &s), ["b", "c", "e", "f"]);
    s.insert("b".into(), NodeStatus::Succeeded);
    assert!(!ready(&g, &s).contains(&"d".to_string()), "d waits for c");
}

#[test]
fn parallel_cap_counts_running_nodes() {
    let g = graph(json!({"max_parallel": 2}), &[]);
    assert_eq!(pick(&g, &all_pending(), 5, None), ["a", "e"]);
    let mut s = all_pending();
    s.insert("a".into(), NodeStatus::Running);
    assert_eq!(pick(&g, &s, 5, None), ["e"]);
    s.insert("e".into(), NodeStatus::Running);
    assert!(pick(&g, &s, 5, None).is_empty());
    // A blocked (waiting) node holds no slot.
    s.insert("e".into(), NodeStatus::Waiting);
    assert_eq!(pick(&g, &s, 5, None), ["f"]);
}

#[test]
fn resource_tags_and_exclusive() {
    let g = graph(
        json!({"max_parallel": 4, "resources": {"gpu": 1}}),
        &[("a", &["gpu"]), ("e", &["gpu"]), ("f", &["lab"])],
    );
    assert_eq!(pick(&g, &all_pending(), 4, None), ["a", "f"]);
    let g = graph(json!({"max_parallel": 4}), &[("e", &["exclusive"])]);
    // e waits for an empty floor; nothing of lower priority starts meanwhile.
    let mut s = all_pending();
    s.insert("a".into(), NodeStatus::Running);
    assert!(pick(&g, &s, 4, None).is_empty());
    s.insert("a".into(), NodeStatus::Succeeded);
    // b and c (priority 2) start; e (exclusive) then waits for the floor.
    assert_eq!(pick(&g, &s, 4, None), ["b", "c"]);
    let s = st(&[("e", NodeStatus::Pending), ("f", NodeStatus::Pending)]);
    assert_eq!(pick(&g, &s, 4, None), ["e"]);
}

#[test]
fn retry_goes_first_and_dependents_reopen() {
    let g = graph(json!({"max_parallel": 1}), &[]);
    let mut s = all_pending();
    s.insert("a".into(), NodeStatus::Succeeded);
    assert_eq!(pick(&g, &s, 1, Some("c")), ["c"]);
    let s = st(&[
        ("a", NodeStatus::Failed),
        ("b", NodeStatus::Skipped),
        ("c", NodeStatus::Skipped),
        ("d", NodeStatus::Skipped),
        ("e", NodeStatus::Pending),
    ]);
    assert_eq!(skipped_dependents(&g, &s, "a"), ["b", "c", "d"]);
    let s = st(&[
        ("a", NodeStatus::Failed),
        ("b", NodeStatus::Pending),
        ("d", NodeStatus::Pending),
    ]);
    assert_eq!(open_dependents(&g, &s, "a"), ["b", "d"]);
}

#[test]
fn settled_and_stranded() {
    let g = graph(json!({}), &[]);
    assert!(!settled(&all_pending()));
    assert!(settled(&st(&[
        ("a", NodeStatus::Succeeded),
        ("b", NodeStatus::Skipped)
    ])));
    assert!(!settled(&st(&[("a", NodeStatus::Waiting)])));
    // b pending with a cancelled upstream and nothing running: stranded.
    let s = st(&[("a", NodeStatus::Cancelled), ("b", NodeStatus::Pending)]);
    assert_eq!(stranded(&g, &s), ["b"]);
    let s = st(&[("a", NodeStatus::Waiting), ("b", NodeStatus::Pending)]);
    assert!(stranded(&g, &s).is_empty());
    assert_eq!(order(&g).first().map(String::as_str), Some("a"));
}

//! The workflow as the scheduler receives it (deterministic text).
//!
//! First line: global limits from `graph.settings`. Then the nodes in
//! topological order (Kahn, ties by id; on a cycle graph order with a warning
//! line first). Prompt-mode nodes carry their prompt; brief-mode nodes carry the
//! body of their brief (`prompt::render_brief` without the header, the upstream
//! inputs and the finish protocol), with its `## ` headings demoted to `#### `.

use crate::contract::{Graph, Node};
use crate::graph_util::{topo_order, upstream_map};
use crate::prompt::{is_simple, render_brief};

/// Brief section that does not belong in the workflow.
const INPUTS_SECTION: &str = "## Inputs from upstream nodes";
/// The finish protocol: the last brief section, cut with everything after it
/// (its example contains `## Deliverables` / `## Status:` lines).
const FINISH_SECTION: &str = "## How to finish";
/// The deliverables note of the brief points at its (dropped) header.
const DELIVERABLES_NOTE: (&str, &str) = (
    "(each path relative to the deliverables folder above)",
    "(each path relative to `nodes/<id>/deliverables/`)",
);
/// Graph folder placeholder for brief bodies (the header that uses it is dropped).
const BRIEF_DIR: &str = ".";

fn limits_line(graph: &Graph) -> String {
    let pools: Vec<String> = graph
        .settings
        .resources
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    let pools = if pools.is_empty() {
        "none".to_string()
    } else {
        pools.join(", ")
    };
    format!(
        "Global limit: max_parallel = {}; resource pools: {pools}",
        graph.settings.max_parallel
    )
}

/// The brief of a structured node minus header, inputs and finish protocol.
fn brief_body(graph: &Graph, node: &Node) -> String {
    let brief =
        render_brief(BRIEF_DIR, graph, node, &[]).replace(DELIVERABLES_NOTE.0, DELIVERABLES_NOTE.1);
    let kept = brief.lines().take_while(|l| l.trim_end() != FINISH_SECTION);
    let mut sections: Vec<Vec<&str>> = Vec::new();
    for line in kept {
        match sections.last_mut() {
            Some(current) if !line.starts_with("## ") => current.push(line),
            None if !line.starts_with("## ") => {} // header before the first section
            _ => sections.push(vec![line]),
        }
    }
    sections
        .into_iter()
        .filter(|s| s[0].trim_end() != INPUTS_SECTION)
        .map(|s| {
            let heading = format!("##{}", s[0]);
            std::iter::once(heading.as_str())
                .chain(s[1..].iter().copied())
                .collect::<Vec<_>>()
                .join("\n")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn prompt_body(node: &Node) -> String {
    let prompt = match node.prompt.trim() {
        "" => "(no prompt)".to_string(),
        p => p.to_string(),
    };
    match node.extra_context.trim() {
        "" => prompt,
        extra => format!("{prompt}\n\nExtra context:\n{extra}"),
    }
}

fn render_node(graph: &Graph, node: &Node, depends: &[String]) -> String {
    let name = match node.title.trim() {
        "" => node.id.as_str(),
        t => t,
    };
    let depends = if depends.is_empty() {
        "nothing".to_string()
    } else {
        depends.join(", ")
    };
    let resources: Vec<&str> = node
        .resources
        .iter()
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
        .collect();
    let deliverables: Vec<String> = node
        .deliverables
        .iter()
        .map(|d| d.path.trim())
        .filter(|p| !p.is_empty())
        .map(|p| format!("{p} (required)"))
        .collect();
    let working_dir = node
        .working_dir
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty());
    let lines: Vec<String> = [
        Some(format!("### Node `{}`: {name}", node.id)),
        Some(format!("- depends on: {depends}")),
        (!resources.is_empty()).then(|| format!("- resources: {}", resources.join(", "))),
        working_dir.map(|w| format!("- working dir: {w}")),
        (!deliverables.is_empty()).then(|| format!("- deliverables: {}", deliverables.join(", "))),
    ]
    .into_iter()
    .flatten()
    .collect();
    let body = if is_simple(node) {
        prompt_body(node)
    } else {
        brief_body(graph, node)
    };
    format!("{}\n\n{body}", lines.join("\n"))
}

/// The workflow text sent to the scheduler.
pub fn render_workflow(graph: &Graph) -> String {
    let (order, warning) = match topo_order(graph) {
        Ok(order) => (order, None),
        Err(_) => (
            graph.nodes.iter().map(|n| n.id.clone()).collect(),
            Some("Warning: the graph has a cycle".to_string()),
        ),
    };
    let upstream = upstream_map(graph);
    let nodes = order.iter().filter_map(|id| {
        let node = graph.nodes.iter().find(|n| &n.id == id)?;
        let depends: Vec<String> = upstream
            .get(id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        Some(render_node(graph, node, &depends))
    });
    warning
        .into_iter()
        .chain([limits_line(graph)])
        .chain(nodes)
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{Deliverable, Edge};

    fn node(id: &str, prompt: &str) -> Node {
        serde_json::from_value(serde_json::json!({ "id": id, "prompt": prompt })).unwrap()
    }

    fn graph(nodes: Vec<Node>, edges: &[(&str, &str)]) -> Graph {
        Graph {
            version: 1,
            name: "g".into(),
            settings: Default::default(),
            nodes,
            edges: edges
                .iter()
                .enumerate()
                .map(|(i, (f, t))| Edge {
                    id: format!("e{i}"),
                    from: f.to_string(),
                    to: t.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn two_nodes_render_in_dependency_order() {
        // Declared out of order; b depends on a.
        let mut b = node("b", "Write the report.");
        b.title = "Report".into();
        b.resources = vec!["cpu".into(), "memory".into()];
        b.working_dir = Some("work".into());
        let g = graph(vec![b, node("a", "Collect data.")], &[("a", "b")]);
        let text = render_workflow(&g);
        assert_eq!(
            text,
            "Global limit: max_parallel = 2; resource pools: cpu=1, gpu=1, memory=1\n\n\
### Node `a`: a\n- depends on: nothing\n\nCollect data.\n\n\
### Node `b`: Report\n- depends on: a\n- resources: cpu, memory\n- working dir: work\n\n\
Write the report."
        );
        assert_eq!(render_workflow(&g), text, "deterministic");
    }

    #[test]
    fn cycle_gives_warning_and_graph_order() {
        let g = graph(
            vec![node("x", "one"), node("y", "two")],
            &[("x", "y"), ("y", "x")],
        );
        let text = render_workflow(&g);
        assert!(text.starts_with("Warning: the graph has a cycle\n\nGlobal limit:"));
        let x = text.find("### Node `x`").unwrap();
        let y = text.find("### Node `y`").unwrap();
        assert!(x < y);
        assert!(text.contains("- depends on: y"));
    }

    #[test]
    fn brief_mode_node_renders_body_without_inputs_or_finish() {
        let mut n = node("design", "Use the survey.");
        n.goal = "Rules file exists.".into();
        n.acceptance = "Script passes.".into();
        n.deliverables = vec![Deliverable {
            path: "rules.md".into(),
            description: "the rules".into(),
        }];
        n.do_nots = vec!["Do not install packages".into()];
        let g = graph(vec![node("survey", "Survey."), n], &[("survey", "design")]);
        let text = render_workflow(&g);
        let body = &text[text.find("### Node `design`").unwrap()..];
        assert!(body.contains("- depends on: survey"));
        assert!(body.contains("- deliverables: rules.md (required)"));
        assert!(body.contains("#### Task\nUse the survey."));
        assert!(body.contains("#### Goal\nRules file exists."));
        assert!(body.contains("#### Do not\n- Do not install packages"));
        assert!(body.contains(
            "- rules.md — the rules
(each path relative to `nodes/<id>/deliverables/`)"
        ));
        for gone in [
            "Inputs from upstream",
            "How to finish",
            "## Status",
            "# Node:",
        ] {
            assert!(
                !body.contains(gone),
                "{gone} must not be rendered:
{body}"
            );
        }
        assert!(
            !body.lines().any(|l| l.starts_with("## ")),
            "brief headings are demoted"
        );
    }

    #[test]
    fn prompt_mode_without_prompt_and_with_extra_context() {
        let mut n = node("a", "");
        n.extra_context = "Use metric units.".into();
        let text = render_workflow(&graph(vec![n], &[]));
        assert!(text.ends_with("(no prompt)\n\nExtra context:\nUse metric units."));
    }
}

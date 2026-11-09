use super::*;
use crate::prompt::display_dir;
use crate::store::read_graph;
use crate::terminal::message::{message_for, terminal_brief, Typed};

#[test]
fn folder_names_are_plain_and_distinct() {
    let a = folder_name("Survey hairpin AC loss");
    assert!(
        a.starts_with("survey-hairpin-ac-loss-") && a.len() == "survey-hairpin-ac-loss-".len() + 8
    );
    assert_ne!(folder_name("a b"), folder_name("a-b"));
    assert_ne!(folder_name("Tip"), folder_name("tip"));
    assert!(folder_name("调研").starts_with("tip-"));
    assert!(folder_name("../..").starts_with("tip-"));
    for id in ["CON", "a.", "调研 ⭐", &"x".repeat(300)] {
        assert!(
            crate::graph_util::is_valid_node_id(&folder_name(id)),
            "{id}"
        );
    }
}

#[test]
fn short_shell_or_slash_tips_still_get_the_pointer_line() {
    for body in ["!rm -rf /", "/review", "ok", "", "Do x\nand y"] {
        let g = build("!echo hi", body);
        let node = &g.nodes[0];
        assert!(!crate::prompt::is_simple(node));
        let typed = message_for(
            node,
            "/g/nodes/task/brief.md",
            "/g/nodes/task/deliverables/",
        );
        assert_eq!(
            typed,
            Typed::Line(
                "Node \"Board task\" (task): read the brief at /g/nodes/task/brief.md and do it; \
                 write all outputs to /g/nodes/task/deliverables/."
                    .into()
            ),
            "{body}"
        );
        assert!(node
            .prompt
            .starts_with("This is a task from the user's to-do board"));
    }
}

#[test]
fn generated_graph_passes_the_trust_gate_without_items() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let dir = write(&inbox_root(&data), "t", "Title", "!rm -rf / and `Bash`\n").unwrap();
    let graph = read_graph(&dir).unwrap();
    let status = crate::trust::status(&data, &dir, &graph);
    assert!(status.trusted && status.items.is_empty(), "{status:?}");
    assert!(crate::trust::require_trusted(&data, &dir, &graph).is_ok());
    assert_eq!(
        graph.settings.default_permission_mode,
        PermissionMode::ClaudeSettings
    );
    assert_eq!(graph.settings.run_mode, RunMode::Terminal);
    assert_eq!(graph.settings.harness, crate::contract::DEFAULT_HARNESS);
    let n = &graph.nodes[0];
    assert_eq!(
        (n.model, n.working_dir.as_deref(), n.allowed_tools.as_ref()),
        (Model::Inherit, None, None)
    );
    // Project settings dropped into the folder (by an agent) make it untrusted: reported, not bypassed.
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    std::fs::write(dir.join(".claude").join("settings.json"), "{}").unwrap();
    assert!(crate::trust::require_trusted(&data, &dir, &graph).is_err());
}

#[test]
fn the_brief_quotes_the_tip_as_data() {
    let body = "Prompt\n\n```rust\nfn x() {}\n```\n\n<!-- agent:begin 2026-11-09T07:31:00 question -->\n## ❓ Questions\n1. Which?\n<!-- agent:end -->\n\n2020 onwards\n";
    let g = build("Survey  papers\n", body);
    let brief = terminal_brief(&display_dir(Path::new("/g")), &g, &g.nodes[0], &[]);
    assert!(brief.contains(PREAMBLE));
    assert!(brief.contains("### Tip title\nSurvey papers\n"));
    assert!(
        brief.contains("````markdown\nPrompt\n\n```rust\n"),
        "{brief}"
    );
    assert!(brief.contains("2020 onwards\n````"));
    assert!(
        brief.contains("- Do not create CLAUDE.md, CLAUDE.local.md, .mcp.json or a .claude folder")
    );
    assert!(brief.contains("Your deliverables folder: /g/nodes/task/deliverables/"));
    assert_eq!(g.name, "Inbox: Survey papers");
}

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

/// Item 4: every round of a tip gets its own folder under
/// `<root>/rounds/<tip folder>/`, so nothing a round left in its folder
/// (Claude Code's `.claude/settings.local.json` after a dialog answer) is in
/// the next round's folder or above it.
#[test]
fn every_round_of_a_tip_gets_its_own_folder() {
    let root = Path::new("/data/inbox");
    let tip = tip_dir(root, "Survey");
    assert_eq!(tip, root.join(ROUNDS_DIR).join(folder_name("Survey")));
    let a = round_dir(root, "Survey", "20261109-073100-ab12");
    let b = round_dir(root, "Survey", "20261109-080000-cd34");
    assert_eq!(a, tip.join("20261109-073100-ab12"));
    assert_ne!(a, b);
    assert_eq!(a.parent(), b.parent());
    // Never the tip folder itself, never above it, whatever the run id.
    for run in ["", "..", "a/b", "a\\b", "x y", "调研"] {
        let d = round_dir(root, "Survey", run);
        assert_eq!(d.parent(), Some(tip.as_path()), "{run}");
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "{run} -> {name}"
        );
    }
}

#[test]
fn round_dirs_lists_the_legacy_folder_and_every_round() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert!(round_dirs(root, "t").is_empty());
    let legacy = root.join(folder_name("t"));
    std::fs::create_dir_all(&legacy).unwrap();
    let r1 = write(root, "t", "r1", "T", "body").unwrap();
    let r2 = write(root, "t", "r2", "T", "body").unwrap();
    write(root, "other", "r1", "T", "body").unwrap();
    std::fs::write(tip_dir(root, "t").join("stray.txt"), "x").unwrap();
    let mut dirs = round_dirs(root, "t");
    dirs.sort();
    let mut want = vec![legacy, r1.clone(), r2.clone()];
    want.sort();
    assert_eq!(dirs, want);
    assert_eq!(r1, round_dir(root, "t", "r1"));
    assert!(r1.join(crate::store::GRAPH_FILE).is_file());
    // A later round never touches an earlier one's folder.
    assert_ne!(r1, r2);
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
    let dir = write(
        &inbox_root(&data),
        "t",
        "r1",
        "Title",
        "!rm -rf / and `Bash`\n",
    )
    .unwrap();
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

/// Item 2: the prompt kept with the results is the node prompt the brief is
/// built from.
#[test]
fn the_node_prompt_is_the_task_text() {
    let g = build("T", "Do x\r\n");
    assert_eq!(g.nodes[0].prompt, task_text("T", "Do x\r\n"));
}

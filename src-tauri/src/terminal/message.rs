//! What terminal mode types into Claude Code for a node, and the node's brief.
//!
//! - A prompt-mode node whose prompt is one line of at most [`LINE_MAX`] chars:
//!   the prompt verbatim.
//! - A prompt-mode node whose prompt has several lines and at most
//!   [`PASTE_MAX`] chars: the prompt as one bracketed paste (verified with the
//!   real CLI 2.1.x: a two-line paste followed by Enter is submitted as one
//!   prompt; docs/CLAUDE_CLI_NOTES.md).
//! - Anything else: one line pointing at the brief file.
//!
//! Typed text never carries control characters other than the paste's own
//! newlines, so it cannot press keys or end the paste early.

use crate::contract::{Graph, Node};
use crate::prompt::{is_simple, render_brief, UpstreamInput};

/// Longest prompt typed verbatim as one line, in chars.
pub const LINE_MAX: usize = 2000;
/// Longest multi-line prompt pasted verbatim, in chars.
pub const PASTE_MAX: usize = 8000;
/// Bracketed paste start / end (xterm).
pub const PASTE_START: &str = "\u{1b}[200~";
pub const PASTE_END: &str = "\u{1b}[201~";
/// Heading of the brief section terminal mode leaves out (no status protocol).
const FINISH_HEADING: &str = "## How to finish\n";

/// The text for one node, before the final Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// One line, typed as is.
    Line(String),
    /// Several lines, sent as one bracketed paste.
    Paste(String),
}

impl Typed {
    /// Bytes written to the pty (without the Enter, sent separately).
    pub fn payload(&self) -> String {
        match self {
            Typed::Line(s) => s.clone(),
            Typed::Paste(s) => format!("{PASTE_START}{s}{PASTE_END}"),
        }
    }

    /// The text as the user reads it.
    pub fn text(&self) -> &str {
        match self {
            Typed::Line(s) | Typed::Paste(s) => s,
        }
    }
}

/// Control characters become spaces; tabs too (Tab is a key in the TUI).
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Lines joined with `\n`; other control characters become spaces.
fn paste_text(s: &str) -> String {
    s.replace("\r\n", "\n")
        .lines()
        .map(one_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn title_of(node: &Node) -> &str {
    if node.title.trim().is_empty() {
        &node.id
    } else {
        node.title.trim()
    }
}

/// The message for `node`. `brief` and `deliverables` are absolute display paths.
pub fn message_for(node: &Node, brief: &str, deliverables: &str) -> Typed {
    let prompt = node.prompt.trim();
    if is_simple(node) && !prompt.is_empty() {
        let chars = prompt.chars().count();
        if !prompt.contains(['\n', '\r']) && chars <= LINE_MAX {
            return Typed::Line(one_line(prompt));
        }
        if chars <= PASTE_MAX {
            return Typed::Paste(paste_text(prompt));
        }
    }
    Typed::Line(one_line(&format!(
        "Node \"{}\" ({}): read the brief at {brief} and do it; write all outputs to {deliverables}.",
        title_of(node),
        node.id
    )))
}

/// The node brief without the "## How to finish" section (terminal mode has
/// no status protocol); deliverables folder line and upstream inputs stay.
pub fn terminal_brief(
    graph_dir: &str,
    graph: &Graph,
    node: &Node,
    upstream: &[UpstreamInput],
) -> String {
    let brief = render_brief(graph_dir, graph, node, upstream);
    // The finish section is always last; rfind skips any copy inside the prompt.
    match brief.rfind(FINISH_HEADING) {
        Some(i) => brief[..i].trim_end().to_string() + "\n",
        None => brief,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Deliverable;
    use serde_json::json;

    fn node(prompt: &str) -> Node {
        serde_json::from_value(json!({"id": "n1", "title": "First", "prompt": prompt})).unwrap()
    }

    #[test]
    fn short_single_line_prompt_is_typed_verbatim() {
        let n = node("  Reply with ok\t please ");
        assert_eq!(
            message_for(&n, "/b.md", "/d/"),
            Typed::Line("Reply with ok  please".into())
        );
        let slash = node("/review the code");
        assert_eq!(
            message_for(&slash, "/b.md", "/d/").text(),
            "/review the code"
        );
    }

    #[test]
    fn multi_line_prompt_is_pasted_and_long_prompts_point_at_the_brief() {
        let n = node("line one\r\nline two\u{1b}[201~");
        let typed = message_for(&n, "/b.md", "/d/");
        assert_eq!(typed, Typed::Paste("line one\nline two [201~".into()));
        assert_eq!(
            typed.payload(),
            "\u{1b}[200~line one\nline two [201~\u{1b}[201~"
        );
        let long = node(&"x".repeat(LINE_MAX + 1));
        assert!(matches!(
            message_for(&long, "/b.md", "/d/"),
            Typed::Paste(_)
        ));
        let huge = node(&"x\n".repeat(PASTE_MAX));
        assert_eq!(
            message_for(&huge, "/g/nodes/n1/brief.md", "/g/nodes/n1/deliverables/").text(),
            "Node \"First\" (n1): read the brief at /g/nodes/n1/brief.md and do it; \
             write all outputs to /g/nodes/n1/deliverables/."
        );
    }

    #[test]
    fn structured_or_empty_nodes_point_at_the_brief() {
        let mut n = node("short");
        n.deliverables = vec![Deliverable {
            path: "a.md".into(),
            description: String::new(),
        }];
        assert!(message_for(&n, "/b.md", "/d/")
            .text()
            .starts_with("Node \"First\" (n1): read"));
        let empty = node("  ");
        assert!(message_for(&empty, "/b.md", "/d/").text().contains("/b.md"));
    }

    #[test]
    fn terminal_brief_has_no_finish_section() {
        let g: crate::contract::Graph =
            serde_json::from_str(include_str!("../../../examples/hello-pipeline/graph.json"))
                .unwrap();
        for n in &g.nodes {
            let full = render_brief("/g", &g, n, &[]);
            let brief = terminal_brief("/g", &g, n, &[]);
            assert!(full.contains(FINISH_HEADING));
            assert!(!brief.contains(FINISH_HEADING), "{brief}");
            assert!(!brief.contains("## Status:"));
            assert!(brief.contains("Your deliverables folder: /g/nodes/"));
            assert!(brief.contains("## Inputs from upstream nodes"));
        }
    }
}

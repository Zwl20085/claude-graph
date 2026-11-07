//! The scheduler's final reply as journal sections.
//!
//! Rules: split on lines matching `^##\s+(.+?)\s*$` (never `###`). The head
//! (case-insensitive, before an optional `:`) picks the kind: `stage` and
//! `decision` keep the text after `:` as title; `question`, `note`, `final`
//! have no title (text after their `:` becomes the first line of the body);
//! any other heading is a note titled with the whole heading. Text before the
//! first heading is an untitled note. Blank sections are dropped. A reply
//! without headings is one note with the whole text.

use std::sync::LazyLock;

use regex::Regex;

use crate::contract::{AssistantKind, AssistantStatus};

static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+(.+?)\s*$").expect("valid regex"));

/// `(kind, title, body)` of one reply section; body trimmed.
pub type Section = (AssistantKind, Option<String>, String);

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Kind, title and a body prefix (text after `:` of an untitled kind).
fn classify(heading: &str) -> (AssistantKind, Option<String>, Option<String>) {
    let (head, rest) = match heading.split_once(':') {
        Some((h, r)) => (h.trim(), Some(r)),
        None => (heading.trim(), None),
    };
    let after = rest.and_then(non_empty);
    match head.to_ascii_lowercase().as_str() {
        "stage" => (AssistantKind::Stage, after, None),
        "decision" => (AssistantKind::Decision, after, None),
        "question" => (AssistantKind::Question, None, after),
        "note" => (AssistantKind::Note, None, after),
        "final" => (AssistantKind::Final, None, after),
        _ => (AssistantKind::Note, non_empty(heading), None),
    }
}

fn section(kind: AssistantKind, title: Option<String>, body: &[&str]) -> Option<Section> {
    let text = body.join("\n").trim().to_string();
    (!text.is_empty()).then_some((kind, title, text))
}

/// Parse a reply into sections, in order.
pub fn parse_reply(text: &str) -> Vec<Section> {
    let lines: Vec<&str> = text.lines().collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| HEADING.is_match(l))
        .map(|(i, _)| i)
        .collect();
    let Some(&first) = starts.first() else {
        return section(AssistantKind::Note, None, &lines)
            .into_iter()
            .collect();
    };
    let preamble = section(AssistantKind::Note, None, &lines[..first]);
    let ends = starts.iter().skip(1).copied().chain([lines.len()]);
    let sections = starts.iter().zip(ends).filter_map(|(&start, end)| {
        let heading = HEADING
            .captures(lines[start])
            .and_then(|c| c.get(1))
            .map_or("", |m| m.as_str());
        let (kind, title, prefix) = classify(heading);
        let body: Vec<&str> = prefix
            .as_deref()
            .into_iter()
            .chain(lines[start + 1..end].iter().copied())
            .collect();
        section(kind, title, &body)
    });
    preamble.into_iter().chain(sections).collect()
}

/// Status after a successful turn: any question → waiting; else any final →
/// done; else idle.
pub fn status_after(sections: &[Section]) -> AssistantStatus {
    let has = |k: AssistantKind| sections.iter().any(|(kind, _, _)| *kind == k);
    if has(AssistantKind::Question) {
        AssistantStatus::Waiting
    } else if has(AssistantKind::Final) {
        AssistantStatus::Done
    } else {
        AssistantStatus::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AssistantKind::*;

    fn s(kind: AssistantKind, title: Option<&str>, text: &str) -> Section {
        (kind, title.map(str::to_string), text.to_string())
    }

    #[test]
    fn mixed_reply_with_all_five_kinds() {
        let reply =
            "Intro line.\n\n## Stage: survey\nWrote survey.md.\n### sub-heading\nstays in body\n\n\
## Decision: Order\nRun design first.\n## Note\nFYI.\n## Question\nProceed?\n## Final\nAll done.\n";
        assert_eq!(
            parse_reply(reply),
            vec![
                s(Note, None, "Intro line."),
                s(
                    Stage,
                    Some("survey"),
                    "Wrote survey.md.\n### sub-heading\nstays in body"
                ),
                s(Decision, Some("Order"), "Run design first."),
                s(Note, None, "FYI."),
                s(Question, None, "Proceed?"),
                s(Final, None, "All done."),
            ]
        );
    }

    #[test]
    fn heads_are_case_insensitive_and_titles_trimmed() {
        assert_eq!(
            parse_reply("##   STAGE :  design  \nok\r\n## final\r\nyes"),
            vec![s(Stage, Some("design"), "ok"), s(Final, None, "yes")]
        );
        assert_eq!(parse_reply("## Stage:\nbody"), vec![s(Stage, None, "body")]);
        assert_eq!(
            parse_reply("## Question: which budget?\nA or B"),
            vec![s(Question, None, "which budget?\nA or B")]
        );
    }

    #[test]
    fn unknown_heading_is_a_titled_note() {
        assert_eq!(
            parse_reply("## Results table\n| a |"),
            vec![s(Note, Some("Results table"), "| a |")]
        );
    }

    #[test]
    fn blank_sections_and_preamble_are_dropped() {
        assert_eq!(
            parse_reply("\n  \n## Note\n\n## Final\ndone"),
            vec![s(Final, None, "done")]
        );
    }

    #[test]
    fn no_headings_is_one_note_and_empty_is_nothing() {
        assert_eq!(
            parse_reply("  just text\n### not a heading\n"),
            vec![s(Note, None, "just text\n### not a heading")]
        );
        assert!(parse_reply("").is_empty());
        assert!(parse_reply(" \n ").is_empty());
        assert!(parse_reply("##nospace").len() == 1);
    }

    #[test]
    fn status_from_sections() {
        assert_eq!(status_after(&[]), AssistantStatus::Idle);
        assert_eq!(status_after(&[s(Note, None, "x")]), AssistantStatus::Idle);
        assert_eq!(status_after(&[s(Final, None, "x")]), AssistantStatus::Done);
        assert_eq!(
            status_after(&[s(Final, None, "x"), s(Question, None, "?")]),
            AssistantStatus::Waiting
        );
    }
}

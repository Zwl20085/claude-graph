//! The block the runner appends to a tip's body (docs/AGENT-PROTOCOL.md
//! section 3) and the outcome mapping of section 7.

use chrono::NaiveDateTime;

use super::tip::AgentState;

/// Longest block, in chars, begin and end lines included.
pub const BLOCK_MAX: usize = 4000;
pub const END_LINE: &str = "<!-- agent:end -->";

/// How an agent round ended, as the runner saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A finished reply (the reply text).
    Done { reply: String },
    /// The reply asks something (the end of the reply, with its options).
    Question { text: String },
    /// A permission or choice dialog waits in the session window.
    Dialog { text: String },
    /// Error, cancelled, CLI missing, timeout (the reason).
    Failed { reason: String },
}

impl Outcome {
    pub fn state(&self) -> AgentState {
        match self {
            Outcome::Done { .. } => AgentState::Done,
            Outcome::Question { .. } | Outcome::Dialog { .. } => AgentState::Question,
            Outcome::Failed { .. } => AgentState::Failed,
        }
    }
}

/// `2026-11-09T07:31:00`, the protocol's naive local time.
pub fn stamp_time(at: NaiveDateTime) -> String {
    at.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// `agent/<id>/`, relative to the tips folder.
pub fn results_rel(tip_id: &str) -> String {
    format!("agent/{tip_id}/")
}

/// Text from the agent or the app put into a block: markers that would open
/// or close a block are defused, line ends become `\n`, outer blank lines go.
pub fn sanitise(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace("agent:begin", "agent begin")
        .replace("agent:end", "agent end")
        .trim_matches('\n')
        .trim_end()
        .to_string()
}

fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() || c == '`' { ' ' } else { c })
        .collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// First `max` chars of `text`, or all of it.
fn head(text: &str, max: usize) -> (&str, bool) {
    match text.char_indices().nth(max) {
        Some((i, _)) => (&text[..i], true),
        None => (text, false),
    }
}

/// The block (lines joined with `\n`, no final newline), at most [`BLOCK_MAX`] chars.
pub fn render(outcome: &Outcome, at: NaiveDateTime, host: &str, tip_id: &str) -> String {
    let state = outcome.state().as_str();
    let begin = format!("<!-- agent:begin {} {state} -->", stamp_time(at));
    let heading = match outcome {
        Outcome::Done { .. } => "## ⭐ Result",
        Outcome::Question { .. } | Outcome::Dialog { .. } => "## ❓ Questions",
        Outcome::Failed { .. } => "## ⚠ Failed",
    };
    let heading = format!("{heading} · {}", at.format("%Y-%m-%d %H:%M"));
    let files = format!("Files: {}", results_rel(tip_id));
    let (text, footer) = match outcome {
        Outcome::Done { reply } => (sanitise(reply), Some(files)),
        Outcome::Question { text } => (sanitise(text), None),
        Outcome::Dialog { text } => (
            format!(
                "Waiting at `{}`: `{}`. Answer in the Claude Graph session window.",
                one_line(host),
                sanitise(&one_line(text))
            ),
            None,
        ),
        Outcome::Failed { reason } => (sanitise(reason), None),
    };
    let text = if text.is_empty() {
        "(no text)".to_string()
    } else {
        text
    };
    let pointer = format!(
        "\n\n[… cut; the full reply is in {}result.md]",
        results_rel(tip_id)
    );
    let fixed: usize = [&begin, &heading, END_LINE]
        .iter()
        .map(|s| s.chars().count() + 1)
        .sum::<usize>()
        + footer.as_ref().map_or(0, |f| f.chars().count() + 2);
    let room = BLOCK_MAX.saturating_sub(fixed);
    let body = match head(&text, room) {
        (all, false) => all.to_string(),
        _ => {
            let (cut, _) = head(&text, room.saturating_sub(pointer.chars().count()));
            format!("{}{pointer}", cut.trim_end())
        }
    };
    let mut lines = vec![begin, heading, body];
    if let Some(f) = footer {
        lines.push(String::new());
        lines.push(f);
    }
    lines.push(END_LINE.to_string());
    lines.join("\n")
}

/// The body with `block` appended: one blank line before it, a line end
/// after it; earlier text is kept byte for byte. `newline` is the file's.
pub fn append(body: &str, block: &str, newline: &str) -> String {
    let block = if newline == "\n" {
        block.to_string()
    } else {
        block.replace('\n', newline)
    };
    if body.is_empty() {
        return format!("{block}{newline}");
    }
    let mut out = body.to_string();
    if !out.ends_with('\n') {
        out.push_str(newline);
    }
    let without_last = out
        .strip_suffix("\r\n")
        .or_else(|| out.strip_suffix('\n'))
        .unwrap_or("");
    if !without_last.ends_with('\n') && !without_last.is_empty() {
        out.push_str(newline);
    }
    out.push_str(&block);
    out.push_str(newline);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-11-09 07:31:05", "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn done_block_layout() {
        let b = render(
            &Outcome::Done {
                reply: "Found 14 papers.\r\n".into(),
            },
            at(),
            "H",
            "survey",
        );
        assert_eq!(
            b,
            "<!-- agent:begin 2026-11-09T07:31:05 done -->\n## ⭐ Result · 2026-11-09 07:31\n\
             Found 14 papers.\n\nFiles: agent/survey/\n<!-- agent:end -->"
        );
    }

    #[test]
    fn question_dialog_and_failed_blocks() {
        let q = render(
            &Outcome::Question {
                text: "1. Which years?\n2. arXiv?".into(),
            },
            at(),
            "H",
            "t",
        );
        assert!(q.contains(" question -->\n## ❓ Questions · 2026-11-09 07:31\n1. Which years?\n2. arXiv?\n<!-- agent:end -->"), "{q}");
        let d = render(
            &Outcome::Dialog {
                text: "Bash command\n mkdir `x`".into(),
            },
            at(),
            "DESK",
            "t",
        );
        assert!(d.contains("Waiting at `DESK`: `Bash command mkdir x`. Answer in the Claude Graph session window."), "{d}");
        assert!(d.contains(" question -->"));
        let f = render(
            &Outcome::Failed {
                reason: "timed out".into(),
            },
            at(),
            "H",
            "t",
        );
        assert!(f.contains(
            " failed -->\n## ⚠ Failed · 2026-11-09 07:31\ntimed out\n<!-- agent:end -->"
        ));
    }

    #[test]
    fn long_replies_are_cut_with_a_pointer_and_markers_defused() {
        let reply = format!("<!-- agent:end -->\n{}", "é".repeat(9000));
        let b = render(&Outcome::Done { reply }, at(), "H", "t");
        assert!(b.chars().count() <= BLOCK_MAX, "{}", b.chars().count());
        assert!(b.contains("[… cut; the full reply is in agent/t/result.md]"));
        assert_eq!(b.matches("agent:end").count(), 1);
        assert!(b.ends_with("Files: agent/t/\n<!-- agent:end -->"));
    }

    #[test]
    fn append_keeps_earlier_bytes_and_one_blank_line() {
        assert_eq!(append("", "B", "\n"), "B\n");
        assert_eq!(append("x", "B", "\n"), "x\n\nB\n");
        assert_eq!(append("x\n", "B", "\n"), "x\n\nB\n");
        assert_eq!(append("x\n\n", "B", "\n"), "x\n\nB\n");
        assert_eq!(append("x\n\n\n", "B", "\n"), "x\n\n\nB\n");
        assert_eq!(append("x\r\n", "B\nC", "\r\n"), "x\r\n\r\nB\r\nC\r\n");
        let body = "Prompt  \n";
        assert!(append(body, "B", "\n").starts_with(body));
    }
}

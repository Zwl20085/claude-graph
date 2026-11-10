//! The block the runner appends to a tip's body (docs/AGENT-PROTOCOL.md
//! section 3) and the outcome mapping of section 7.

use chrono::NaiveDateTime;

use super::results::{REPLY_NAME, RESULTS_FOLDER_NAME};
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

/// Chars of `text` once each `\n` is written as a line end of `nl` chars.
fn cost(text: &str, nl: usize) -> usize {
    text.chars().count() + text.matches('\n').count() * nl.saturating_sub(1)
}

/// The longest start of `text` that costs at most `max` chars (each `\n`
/// counted as `nl`), and whether it was cut.
fn head(text: &str, max: usize, nl: usize) -> (&str, bool) {
    let mut used = 0;
    for (i, c) in text.char_indices() {
        used += if c == '\n' { nl } else { 1 };
        if used > max {
            return (&text[..i], true);
        }
    }
    (text, false)
}

/// The block (lines joined with `\n`, no final newline), at most [`BLOCK_MAX`] chars.
///
/// `newline` is the tip file's line end; the cap holds for the block as
/// written (a `\r\n` counts as two chars).
pub fn render(
    outcome: &Outcome,
    at: NaiveDateTime,
    host: &str,
    tip_id: &str,
    newline: &str,
) -> String {
    render_with(outcome, &Footnotes::default(), at, host, tip_id, newline)
}

/// What the runner adds under a done or failed block about the results
/// folder, in its own fixed words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footnotes {
    /// Names of the files that were not copied ([`not_copied_line`]).
    pub not_copied: Vec<String>,
    /// Why `result.md` is the reply as written (not rewritten), if so.
    pub reply_as_written: Option<String>,
}

impl Footnotes {
    /// The reply did not reach the results folder.
    fn reply_lost(&self) -> bool {
        self.not_copied
            .iter()
            .any(|n| n == REPLY_NAME || n == RESULTS_FOLDER_NAME)
    }

    /// The lines, `None` when there are none.
    fn lines(&self, tip_id: &str) -> Option<String> {
        let as_written = self.reply_as_written.as_deref().map(|why| {
            format!(
                "`{REPLY_NAME}` is the reply as written, not rewritten ({}).",
                sanitise(&one_line(why))
            )
        });
        let lines: Vec<String> = not_copied_line(tip_id, &self.not_copied)
            .into_iter()
            .chain(as_written)
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

/// Most file names listed in the "not copied" line.
pub const NOT_COPIED_MAX: usize = 10;
/// Longest file name shown there, in chars.
const NAME_MAX: usize = 80;

/// The fixed-format line naming the files of a done or failed round that
/// were not copied into the results folder (`None`: all were). Names only,
/// in the runner's words: never an OS error or a local path.
pub fn not_copied_line(tip_id: &str, names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let shown: Vec<String> = names
        .iter()
        .take(NOT_COPIED_MAX)
        .map(|n| {
            let n = sanitise(&one_line(n));
            let n = if n.chars().count() > NAME_MAX {
                let cut: String = n.chars().take(NAME_MAX - 1).collect();
                format!("{cut}…")
            } else {
                n
            };
            format!("`{n}`")
        })
        .collect();
    let more = names.len().saturating_sub(NOT_COPIED_MAX);
    let tail = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    Some(format!(
        "Not copied to {}: {}{tail}.",
        results_rel(tip_id),
        shown.join(", ")
    ))
}

/// [`render`], with the runner's [`Footnotes`] under a done or failed block
/// (they are kept whatever the reply's length).
pub fn render_with(
    outcome: &Outcome,
    notes: &Footnotes,
    at: NaiveDateTime,
    host: &str,
    tip_id: &str,
    newline: &str,
) -> String {
    let nl = newline.chars().count().max(1);
    let missing = match outcome {
        Outcome::Done { .. } | Outcome::Failed { .. } => notes.lines(tip_id),
        Outcome::Question { .. } | Outcome::Dialog { .. } => None,
    };
    let reply_lost = notes.reply_lost();
    let state = outcome.state().as_str();
    let begin = format!("<!-- agent:begin {} {state} -->", stamp_time(at));
    let heading = match outcome {
        Outcome::Done { .. } => "## ⭐ Result",
        Outcome::Question { .. } | Outcome::Dialog { .. } => "## ❓ Questions",
        Outcome::Failed { .. } => "## ⚠ Failed",
    };
    let heading = format!("{heading} · {}", at.format("%Y-%m-%d %H:%M"));
    let files = format!("Files: {}", results_rel(tip_id));
    let files = match &missing {
        Some(line) => format!("{files}\n{line}"),
        None => files,
    };
    let (text, footer) = match outcome {
        Outcome::Done { reply } => (sanitise(reply), Some(files)),
        Outcome::Failed { reason } if missing.is_some() => (sanitise(reason), missing),
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
    let pointer = if reply_lost {
        format!(
            "\n\n[… cut; the full reply could not be copied to {}]",
            results_rel(tip_id)
        )
    } else {
        format!(
            "\n\n[… cut; the full reply is in {}result.md]",
            results_rel(tip_id)
        )
    };
    // Every line but the last ends with `newline` (the body's line end is
    // counted with the end line).
    let fixed: usize = [&begin, &heading, END_LINE]
        .iter()
        .map(|s| cost(s, nl) + nl)
        .sum::<usize>()
        + footer.as_ref().map_or(0, |f| cost(f, nl) + 2 * nl);
    let room = BLOCK_MAX.saturating_sub(fixed);
    let body = match head(&text, room, nl) {
        (all, false) => all.to_string(),
        _ => {
            let (cut, _) = head(&text, room.saturating_sub(cost(&pointer, nl)), nl);
            format!("{}{pointer}", cut.trim_end())
        }
    };
    let mut lines = vec![begin, heading, body];
    if let Some(f) = footer {
        lines.push(String::new());
        lines.push(f);
    }
    lines.push(END_LINE.to_string());
    let block = lines.join("\n");
    if newline == "\n" {
        block
    } else {
        block.replace('\n', newline)
    }
}

/// The body with `block` (rendered with the file's line end `newline`)
/// appended: one blank line before it, a line end after it; earlier text is
/// kept byte for byte.
pub fn append(body: &str, block: &str, newline: &str) -> String {
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
    out.push_str(block);
    out.push_str(newline);
    out
}

#[cfg(test)]
#[path = "block_tests.rs"]
mod tests;

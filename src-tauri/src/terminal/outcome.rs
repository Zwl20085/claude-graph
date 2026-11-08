//! Does a finished turn ask the user something? A pure classifier over the
//! reply's final text (docs/ARCHITECTURE.md "Turn outcomes").
//!
//! Only the tail counts: the last [`TAIL_LINES`] non-empty lines once fenced
//! code, the `## Deliverables` section and `## Status:` lines are set aside.
//! A tail line is a question when it holds a sentence ending in `?` (outside
//! inline code and quotes; a `?` inside a URL does not end a sentence) or
//! starts with a question stem ("which", "should i", "let me know", …).
//! A completion marker ("## Status: DONE", "Done.", "Completed", "All
//! deliverables written") after the last question makes the turn `Done`.
//! Choices are the numbered / lettered list right after the question line
//! (else the list just above it). When in doubt the classifier says
//! `Question`: an offer ("Want me to also add tests?") pauses the node until
//! the user answers or continues.

use std::sync::OnceLock;

use regex::Regex;

use crate::contract::ReplyChoice;

/// Non-empty lines of the reply's end that are looked at.
pub const TAIL_LINES: usize = 8;
/// A choice label is cut to this many chars.
pub const LABEL_MAX: usize = 80;
/// A question is cut to this many chars.
pub const QUESTION_MAX: usize = 300;
/// `reply_excerpt` is the last this many chars of the reply.
pub const EXCERPT_MAX: usize = 1200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    Done,
    Question,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub kind: OutcomeKind,
    pub question: Option<String>,
    pub choices: Vec<ReplyChoice>,
}

impl Outcome {
    fn done() -> Self {
        Outcome {
            kind: OutcomeKind::Done,
            question: None,
            choices: Vec::new(),
        }
    }
}

/// Stems that make a line a question wherever it ends.
const STRONG_STEMS: [&str; 12] = [
    "let me know",
    "please confirm",
    "please choose",
    "please pick",
    "please tell",
    "please let",
    "can you confirm",
    "can you clarify",
    "can you tell",
    "options:",
    "choose one",
    "pick one",
];
/// Interrogative stems: a question unless the line ends with a period.
const STEMS: [&str; 11] = [
    "which",
    "should i",
    "shall i",
    "do you want",
    "would you like",
    "do you prefer",
    "could you",
    "your call",
    "how would you like",
    "what should",
    "what would",
];

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid outcome regex"))
}

/// Inline code and quoted text (they do not ask anything themselves).
fn quoted() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r#"`[^`]*`|"[^"]*"|\u{201c}[^\u{201d}]*\u{201d}"#)
}

/// A `?` that ends a sentence: followed by the end, a space or closing
/// punctuation / markdown (not by a URL's query text).
fn ends_question() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r#"\?+(?:$|[\s)\]*_'"\u{201d}.!:])"#)
}

/// List item markers: `1.`, `1)`, `a)`, `A.`, `(a)`, `[A]`, `**A)**`,
/// `- **Option A**` (any list), and plain bullets (right after a question).
fn list_item() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"^\s*(?:(?P<num>\d{1,2}[.)])|(?P<let>\(?[A-Ha-h][.)])|\[(?P<br>[A-Ha-h1-9])\]|\*\*(?P<bold>[A-Ha-h1-9][.)])\*\*|(?P<bul>[-*\u{2022}]))\s+(?P<text>.*)$",
    )
}

fn completion() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)^(?:#+\s*)?(?:status:\s*done\b.*|(?:done|completed|all done)[.!]?|.*all deliverables (?:are |have been )?written.*)$",
    )
}

/// One line of the reply and whether it is set aside (code, deliverables).
struct Line<'a> {
    text: &'a str,
    skip: bool,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut fence = false;
    let mut deliverables = false;
    for raw in text.lines() {
        let t = raw.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            out.push(Line {
                text: raw,
                skip: true,
            });
            continue;
        }
        if !fence && t.starts_with('#') {
            let heading = t.trim_start_matches('#').trim().to_lowercase();
            deliverables = heading.starts_with("deliverables");
        }
        let status = t
            .to_lowercase()
            .trim_start_matches('#')
            .trim()
            .starts_with("status:");
        out.push(Line {
            text: raw,
            skip: fence || deliverables || status && !completion().is_match(t),
        });
    }
    out
}

/// Markdown emphasis and quote markers around a line's text.
fn bare(line: &str) -> String {
    line.trim()
        .trim_start_matches('>')
        .trim()
        .trim_matches(|c| c == '*' || c == '_')
        .trim()
        .to_string()
}

fn is_question(line: &str) -> bool {
    let t = bare(line);
    let lower = t.to_lowercase();
    let item = list_item()
        .captures(&t)
        .and_then(|c| c.name("text"))
        .map_or(lower.clone(), |m| m.as_str().to_lowercase());
    let unquoted = quoted().replace_all(&t, " ");
    if ends_question().is_match(&unquoted) {
        return true;
    }
    let stem = |s: &str| lower.starts_with(s) || item.starts_with(s);
    if STRONG_STEMS.iter().any(|s| stem(s)) {
        return true;
    }
    STEMS.iter().any(|s| stem(s)) && !lower.trim_end().ends_with('.')
}

fn cut(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// The text of a list item line without its marker (None: not an item;
/// plain bullets only when `bullets`).
fn item_text(line: &str, bullets: bool) -> Option<String> {
    let c = list_item().captures(line)?;
    let text = c.name("text").map_or("", |m| m.as_str());
    let option = text.to_lowercase().starts_with("**option ");
    if c.name("bul").is_some() && !bullets && !option {
        return None;
    }
    let text = text.replace("**", "");
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Items starting at `from` (consecutive, detail lines indented below them).
fn items_from(lines: &[&str], from: usize, bullets: bool) -> Vec<ReplyChoice> {
    let mut out: Vec<ReplyChoice> = Vec::new();
    for l in lines.iter().skip(from) {
        if let Some(text) = item_text(l, bullets) {
            out.push(ReplyChoice {
                label: cut(&text, LABEL_MAX),
                detail: None,
            });
        } else if let (Some(last), true) = (out.last_mut(), l.starts_with([' ', '\t'])) {
            let d = l.trim();
            last.detail = Some(match &last.detail {
                Some(prev) => format!("{prev}\n{d}"),
                None => d.to_string(),
            });
        } else {
            break;
        }
    }
    out
}

/// The list right above line `q` (its items end at `q`).
fn items_before(lines: &[&str], q: usize) -> Vec<ReplyChoice> {
    let mut start = q;
    while start > 0 {
        let l = lines[start - 1];
        if item_text(l, false).is_some() || l.starts_with([' ', '\t']) {
            start -= 1;
        } else {
            break;
        }
    }
    if start == q {
        return Vec::new();
    }
    items_from(&lines[..q], start, false)
}

/// The question sentence of a question line (its last sentence with `?`).
fn question_text(line: &str) -> String {
    let t = bare(line);
    let t = list_item()
        .captures(&t)
        .and_then(|c| c.name("text"))
        .map_or(t.clone(), |m| m.as_str().to_string());
    cut(&t.replace("**", ""), QUESTION_MAX)
}

/// Classify the final text of a turn (see the module doc).
pub fn classify(reply: &str) -> Outcome {
    let all = lines(reply);
    let kept: Vec<(usize, &str)> = all
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.skip && !l.text.trim().is_empty())
        .map(|(i, l)| (i, l.text))
        .collect();
    let tail = &kept[kept.len().saturating_sub(TAIL_LINES)..];
    let Some(&(q_at, q_line)) = tail.iter().rev().find(|(_, l)| is_question(l)) else {
        return Outcome::done();
    };
    let completed_after = all
        .iter()
        .enumerate()
        .skip(q_at + 1)
        .any(|(_, l)| completion().is_match(l.text.trim()));
    if completed_after {
        return Outcome::done();
    }
    let texts: Vec<&str> = all.iter().map(|l| l.text).collect();
    let mut choices = items_from(&texts, q_at + 1, true);
    if choices.is_empty() {
        choices = items_before(&texts, q_at);
    }
    Outcome {
        kind: OutcomeKind::Question,
        question: Some(question_text(q_line)),
        choices: choices
            .into_iter()
            .filter(|c| !c.label.is_empty())
            .collect(),
    }
}

/// The last [`EXCERPT_MAX`] chars of `reply`.
pub fn excerpt(reply: &str) -> String {
    let t = reply.trim();
    let n = t.chars().count();
    t.chars().skip(n.saturating_sub(EXCERPT_MAX)).collect()
}

#[cfg(test)]
#[path = "outcome_tests.rs"]
mod tests;

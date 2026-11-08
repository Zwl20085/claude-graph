//! Interactive prompts on the live screen (docs/ARCHITECTURE.md "Turn
//! outcomes", screen parsing): an option list drawn after the CLI's last
//! empty input box, with a cursor on one option and/or a footer hint
//! ("Enter to select", "Esc to cancel", "(y/n)", …). Parsed from the rows of
//! the live grid ([`super::live`]) into a [`ScreenDialog`].
//!
//! How a harness draws its dialogs is a [`DialogStyle`] (part of
//! [`super::screen::Markers`]): the option-row regex, the cursor glyphs, the
//! footer hints, the free-text option labels, and whether a digit selects.

use std::sync::Arc;

use regex::Regex;

use crate::contract::{DialogKind, PromptOption};
use crate::terminal::screen::normalize;

/// Claude Code / Codex numbered option rows: optional cursor, number, label.
pub const DEFAULT_OPTION_PATTERN: &str =
    r"^\s*(?:(?P<cur>[\u{276f}>\u{203a}])\s*)?(?P<num>\d{1,2})[.)]\s+(?P<label>\S.*?)\s*$";
/// Footer and question texts that mark a dialog (lowercase).
pub const DEFAULT_HINTS: [&str; 9] = [
    "enter to select",
    "enter to confirm",
    "esc to cancel",
    "tab to",
    "(y/n)",
    "[y/n]",
    "do you want to proceed",
    "to navigate",
    "would you like to run",
];
/// Labels of the option that takes free text (lowercase, no final period).
pub const TEXT_OPTIONS: [&str; 3] = ["type something", "other", "type your own answer"];
/// Words in the question / hints that make a dialog a permission prompt.
const PERMISSION_WORDS: [&str; 4] = [
    "do you want to proceed",
    "would you like to run",
    "allow",
    "permission",
];
/// Screen text after a turn was interrupted at a dialog (Claude Code: no
/// Stop hook follows).
pub const CLAUDE_INTERRUPTED: [&str; 1] = ["what should claude do instead"];
/// Rows looked at, from the bottom of the screen.
const WINDOW_ROWS: usize = 40;
/// Rows above the first option searched for the question.
const QUESTION_ROWS: usize = 4;

/// An option-row regex compared by its pattern.
#[derive(Debug, Clone)]
pub struct OptionPattern(pub Arc<Regex>);

impl PartialEq for OptionPattern {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}
impl Eq for OptionPattern {}

impl OptionPattern {
    /// `pattern` must name the groups `num` (optional) and `label`, and may
    /// name `cur` (the cursor). Invalid: `Err` with the regex error.
    pub fn new(pattern: &str) -> Result<Self, String> {
        let re = Regex::new(pattern).map_err(|e| e.to_string())?;
        if !re.capture_names().any(|n| n == Some("label")) {
            return Err("the option pattern needs a (?P<label>…) group".into());
        }
        Ok(Self(Arc::new(re)))
    }

    pub fn default_pattern() -> Self {
        Self::new(DEFAULT_OPTION_PATTERN).expect("valid default option pattern")
    }
}

/// How one harness draws its dialogs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogStyle {
    pub option: OptionPattern,
    /// Cursor glyphs of the highlighted option.
    pub cursors: Vec<char>,
    /// Footer / question hints (lowercase, normalized).
    pub hints: Vec<String>,
    /// Labels of a free-text option (lowercase).
    pub text_options: Vec<String>,
    /// A digit key selects (and for single choices submits) that option.
    pub digits_select: bool,
    /// Texts showing the turn was interrupted without an end signal.
    pub interrupted: Vec<String>,
}

impl DialogStyle {
    /// The generic style (Codex, custom profiles without their own).
    pub fn generic() -> Self {
        DialogStyle {
            option: OptionPattern::default_pattern(),
            cursors: vec!['\u{276f}', '>', '\u{203a}'],
            hints: DEFAULT_HINTS.iter().map(|h| h.to_string()).collect(),
            text_options: TEXT_OPTIONS.iter().map(|t| t.to_string()).collect(),
            digits_select: false,
            interrupted: Vec::new(),
        }
    }

    /// Claude Code (digits verified 2026-11-08, docs/CLAUDE_CLI_NOTES.md).
    pub fn claude() -> Self {
        DialogStyle {
            digits_select: true,
            interrupted: CLAUDE_INTERRUPTED.iter().map(|t| t.to_string()).collect(),
            ..Self::generic()
        }
    }

    /// A custom profile's style: its own pattern and hints, else generic.
    pub fn custom(pattern: Option<&str>, hints: &[String]) -> Self {
        let base = Self::generic();
        let option = pattern
            .and_then(|p| OptionPattern::new(p).ok())
            .unwrap_or(base.option.clone());
        let mut all = base.hints.clone();
        all.extend(hints.iter().map(|h| normalize(h)).filter(|h| !h.is_empty()));
        DialogStyle {
            option,
            hints: all,
            ..base
        }
    }
}

/// One row of the dialog that the cursor can be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavRow {
    Option(usize),
    /// The "Submit" row of a multi-select question.
    Submit,
}

/// A dialog as the screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenDialog {
    pub kind: DialogKind,
    pub question: String,
    pub header: Option<String>,
    pub options: Vec<PromptOption>,
    /// Checked boxes of a multi-select question.
    pub checked: Vec<bool>,
    /// The row the cursor is on.
    pub cursor: Option<NavRow>,
    /// Rows in cursor order (options, and Submit where it is drawn).
    pub nav: Vec<NavRow>,
    pub multi: bool,
    /// Index of the free-text option, if any.
    pub text_option: Option<usize>,
    pub hinted: bool,
}

impl ScreenDialog {
    /// The highlighted option.
    pub fn selected(&self) -> Option<usize> {
        match self.cursor {
            Some(NavRow::Option(i)) => Some(i),
            _ => None,
        }
    }

    /// Identity of the dialog (question and labels), to tell a new dialog
    /// from the same one redrawn.
    pub fn signature(&self) -> String {
        let labels: Vec<&str> = self.options.iter().map(|o| o.label.as_str()).collect();
        format!("{}\u{1f}{}", self.question, labels.join("\u{1f}"))
    }
}

fn is_rule(row: &str) -> bool {
    let t = row.trim();
    !t.is_empty()
        && t.chars()
            .all(|c| ('\u{2500}'..='\u{257f}').contains(&c) || c == '\u{fffd}')
}

/// The row is the CLI's empty input box per `idle` markers: a row that is
/// only a prompt glyph, or one holding a non-glyph idle text.
fn is_idle_row(row: &str, idle: &[String]) -> bool {
    let t = row.trim();
    idle.iter().any(|m| {
        let glyph = m.trim_matches(|c: char| c.is_whitespace());
        if glyph.chars().count() == 1 {
            t == glyph
        } else {
            !glyph.is_empty() && normalize(t).contains(glyph)
        }
    })
}

/// Strip a checkbox prefix: `[ ] X` → (`X`, Some(false)), `[✔] X` → Some(true).
fn checkbox(label: &str) -> (String, Option<bool>) {
    let t = label.trim();
    for (prefix, on) in [
        ("[ ]", false),
        ("[\u{2714}]", true),
        ("[x]", true),
        ("[X]", true),
        ("[\u{2713}]", true),
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return (rest.trim().to_string(), Some(on));
        }
    }
    (t.to_string(), None)
}

struct OptionRow {
    row: usize,
    num: Option<usize>,
    label: String,
    cursor: bool,
}

fn option_rows(rows: &[String], style: &DialogStyle) -> Vec<OptionRow> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let c = style.option.0.captures(r)?;
            let label = c.name("label")?.as_str().trim().to_string();
            let num = c.name("num").and_then(|n| n.as_str().parse().ok());
            let cursor = c.name("cur").is_some();
            Some(OptionRow {
                row: i,
                num,
                label,
                cursor,
            })
        })
        .collect()
}

/// The last run of options numbered 1, 2, 3, … (other rows may sit between).
fn numbered_group(found: Vec<OptionRow>) -> Vec<OptionRow> {
    let mut groups: Vec<Vec<OptionRow>> = Vec::new();
    for o in found {
        match (groups.last_mut(), o.num) {
            (Some(g), Some(n)) if g.last().and_then(|l| l.num) == Some(n - 1) && n > 1 => g.push(o),
            (_, Some(1)) => groups.push(vec![o]),
            _ => {}
        }
    }
    groups.pop().unwrap_or_default()
}

fn cursor_at(row: &str, style: &DialogStyle) -> bool {
    row.trim_start()
        .chars()
        .next()
        .is_some_and(|c| style.cursors.contains(&c))
}

fn strip_cursor(row: &str, style: &DialogStyle) -> String {
    row.trim()
        .trim_start_matches(|c| style.cursors.contains(&c))
        .trim()
        .to_string()
}

/// The question: the nearest row above the first option ending with `?` or
/// `:` (else the nearest non-empty, non-rule row); and a header row
/// (`☐ header`, `☒ header`, or the tab strip) just above it.
fn question_above(rows: &[String], first: usize) -> (String, Option<String>) {
    let from = first.saturating_sub(QUESTION_ROWS);
    let candidates: Vec<(usize, &str)> = (from..first)
        .rev()
        .map(|i| (i, rows[i].trim()))
        .filter(|(_, r)| !r.is_empty() && !is_rule(r))
        .collect();
    let picked = candidates
        .iter()
        .find(|(_, r)| r.ends_with('?') || r.ends_with(':'))
        .or_else(|| candidates.first())
        .copied();
    let Some((at, question)) = picked else {
        return (String::new(), None);
    };
    let header = (at.saturating_sub(2)..at)
        .rev()
        .map(|i| rows[i].trim())
        .find_map(|r| {
            let r = r.trim_start_matches('\u{2190}').trim();
            let rest = r
                .strip_prefix('\u{2610}')
                .or_else(|| r.strip_prefix('\u{2612}'))?;
            let h = rest.split("\u{2714} Submit").next().unwrap_or(rest);
            Some(h.trim().trim_end_matches('\u{2192}').trim().to_string())
        });
    (question.to_string(), header.filter(|h| !h.is_empty()))
}

/// The dialog on `rows` (the live screen), if one is drawn after the last
/// empty input box per `idle`. Needs at least two options and either a
/// cursor on one of them or a hint on screen.
pub fn parse(rows: &[String], idle: &[String], style: &DialogStyle) -> Option<ScreenDialog> {
    let start = rows
        .iter()
        .rposition(|r| is_idle_row(r, idle))
        .map_or(0, |i| i + 1);
    let start = start.max(rows.len().saturating_sub(WINDOW_ROWS));
    let rows = &rows[start..];
    let group = numbered_group(option_rows(rows, style));
    if group.len() < 2 {
        return None;
    }
    let first = group[0].row;
    let last = group[group.len() - 1].row;
    let hint_text = normalize(&rows[first.saturating_sub(QUESTION_ROWS)..].join("\n"));
    let hinted = style.hints.iter().any(|h| hint_text.contains(h.as_str()));
    let (question, header) = question_above(rows, first);
    let mut options: Vec<PromptOption> = Vec::new();
    let mut checked = Vec::new();
    let mut nav = Vec::new();
    let mut cursor = None;
    for (k, o) in group.iter().enumerate() {
        let (label, check) = checkbox(&o.label);
        if o.cursor {
            cursor = Some(NavRow::Option(k));
        }
        checked.push(check.unwrap_or(false));
        nav.push(NavRow::Option(k));
        let end = group.get(k + 1).map_or(last + 1, |n| n.row);
        let mut detail: Vec<String> = Vec::new();
        for r in &rows[o.row + 1..end] {
            let plain = strip_cursor(r, style);
            if plain.eq_ignore_ascii_case("submit") {
                if cursor_at(r, style) {
                    cursor = Some(NavRow::Submit);
                }
                nav.push(NavRow::Submit);
            } else if !plain.is_empty() && !is_rule(r) {
                detail.push(plain);
            }
        }
        options.push(PromptOption {
            index: k,
            label,
            detail: (!detail.is_empty()).then(|| detail.join("\n")),
        });
    }
    // A Submit row after the last option (multi-select).
    for r in rows.iter().skip(last + 1).take(2) {
        if strip_cursor(r, style).eq_ignore_ascii_case("submit") && !nav.contains(&NavRow::Submit) {
            if cursor_at(r, style) {
                cursor = Some(NavRow::Submit);
            }
            nav.push(NavRow::Submit);
        }
    }
    if cursor.is_none() && !hinted {
        return None;
    }
    let multi = group.iter().any(|o| checkbox(&o.label).1.is_some());
    let text_option = options.iter().position(|o| {
        let l = o.label.to_lowercase();
        let l = l.trim_end_matches('.');
        style.text_options.iter().any(|t| t == l)
    });
    let lower = format!("{} {hint_text}", question.to_lowercase());
    let kind = if PERMISSION_WORDS.iter().any(|w| lower.contains(w)) {
        DialogKind::Permission
    } else {
        DialogKind::Choice
    };
    Some(ScreenDialog {
        kind,
        question,
        header,
        options,
        checked,
        cursor,
        nav,
        multi,
        text_option,
        hinted,
    })
}

/// The screen shows that the turn was interrupted (style's texts in `text`,
/// normalized output written since the turn began).
pub fn interrupted(text: &str, style: &DialogStyle) -> bool {
    style.interrupted.iter().any(|m| text.contains(m.as_str()))
}

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod tests;

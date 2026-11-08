//! Answering a dialog with keys (docs/CLAUDE_CLI_NOTES.md "Turn outcomes",
//! verified 2026-11-08 with Claude Code 2.1.319):
//! - a choice: where digits select ([`DialogStyle::digits_select`]) the
//!   digit, then Enter only if the same dialog is still open after
//!   [`DIGIT_SETTLE`]; else Down/Up from the highlighted row, then Enter;
//! - several choices (multi-select): Space on each box to toggle, then the
//!   Submit row and Enter, then "Submit answers" on the review screen;
//! - text: move to the free-text option (its digit, or arrows), type the
//!   text, Enter; a plain text prompt takes the text and Enter.
//!
//! Afterwards the screen must change within [`CHANGE_WAIT`]; if the same
//! dialog still shows with the same highlighted row the answer failed.

use std::future::Future;
use std::time::Duration;

use crate::contract::{AppError, PromptAnswer};
use crate::terminal::deliver::Halt;
use crate::terminal::dialog::{DialogStyle, NavRow, ScreenDialog};
use crate::terminal::message::one_line;
use crate::terminal::prompt_api::ONLY_OPTIONS;

pub const DOWN: &[u8] = b"\x1b[B";
pub const UP: &[u8] = b"\x1b[A";
pub const ENTER: &[u8] = b"\r";
pub const SPACE: &[u8] = b" ";
/// After a digit: wait this long before deciding whether Enter is needed.
pub const DIGIT_SETTLE: Duration = Duration::from_millis(300);
/// Between keys of a sequence.
pub const KEY_GAP: Duration = Duration::from_millis(120);
/// After pressing Submit of a multi-select question (the review screen).
pub const REVIEW_WAIT: Duration = Duration::from_millis(1000);
/// The screen must change within this long.
pub const CHANGE_WAIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(200);
/// Label of the review screen's confirm option (lowercase).
const SUBMIT_ANSWERS: &str = "submit answers";

/// What answering needs from the session: keys, time, the dialog on screen.
pub trait DialogIo {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Halt>;
    fn wait(&mut self, d: Duration) -> impl Future<Output = Result<(), Halt>> + Send;
    /// The dialog on screen now (no quiet needed).
    fn dialog(&self) -> Option<ScreenDialog>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answered {
    /// The screen changed (dialog gone, another dialog, or another row).
    Changed,
    /// The same dialog still shows with the same highlighted row.
    Unchanged,
}

/// Why an answer was not sent.
#[derive(Debug)]
pub enum AnswerError {
    Halt(Halt),
    Invalid(AppError),
}

impl From<Halt> for AnswerError {
    fn from(h: Halt) -> Self {
        AnswerError::Halt(h)
    }
}

fn position(d: &ScreenDialog, row: NavRow) -> Option<usize> {
    d.nav.iter().position(|r| *r == row)
}

/// Arrow keys from nav position `from` to `to`.
pub fn arrows(from: usize, to: usize) -> Vec<u8> {
    let (key, n) = if to >= from {
        (DOWN, to - from)
    } else {
        (UP, from - to)
    };
    key.repeat(n)
}

fn cursor_pos(d: &ScreenDialog) -> usize {
    d.cursor.and_then(|c| position(d, c)).unwrap_or(0)
}

async fn press<I: DialogIo + Send>(io: &mut I, bytes: &[u8]) -> Result<(), Halt> {
    if !bytes.is_empty() {
        io.write(bytes)?;
        io.wait(KEY_GAP).await?;
    }
    Ok(())
}

/// Move the cursor to `row` (arrows from where the screen shows it now).
async fn go_to<I: DialogIo + Send>(io: &mut I, d: &ScreenDialog, row: NavRow) -> Result<(), Halt> {
    let now = io.dialog().filter(|n| n.signature() == d.signature());
    let current = now.as_ref().unwrap_or(d);
    let to = position(current, row).unwrap_or(0);
    press(io, &arrows(cursor_pos(current), to)).await
}

async fn choose<I: DialogIo + Send>(
    io: &mut I,
    d: &ScreenDialog,
    i: usize,
    style: &DialogStyle,
) -> Result<(), Halt> {
    if style.digits_select && i < 9 && Some(i) != d.text_option {
        io.write((i + 1).to_string().as_bytes())?;
        io.wait(DIGIT_SETTLE).await?;
        let still = io.dialog().filter(|n| n.signature() == d.signature());
        if let Some(now) = still {
            go_to(io, &now, NavRow::Option(i)).await?;
            press(io, ENTER).await?;
        }
        return Ok(());
    }
    go_to(io, d, NavRow::Option(i)).await?;
    press(io, ENTER).await
}

async fn choose_many<I: DialogIo + Send>(
    io: &mut I,
    d: &ScreenDialog,
    wanted: &[usize],
) -> Result<(), Halt> {
    for i in 0..d.options.len() {
        let on = d.checked.get(i).copied().unwrap_or(false);
        if on != wanted.contains(&i) {
            go_to(io, d, NavRow::Option(i)).await?;
            press(io, SPACE).await?;
        }
    }
    if position(d, NavRow::Submit).is_some() {
        go_to(io, d, NavRow::Submit).await?;
    }
    press(io, ENTER).await?;
    io.wait(REVIEW_WAIT).await?;
    let review = io.dialog().and_then(|r| {
        let at = r
            .options
            .iter()
            .position(|o| o.label.to_lowercase() == SUBMIT_ANSWERS)?;
        Some((r, at))
    });
    if let Some((r, at)) = review {
        go_to(io, &r, NavRow::Option(at)).await?;
        press(io, ENTER).await?;
    }
    Ok(())
}

async fn type_text<I: DialogIo + Send>(
    io: &mut I,
    d: &ScreenDialog,
    text: &str,
    style: &DialogStyle,
    plain_prompt: bool,
) -> Result<(), AnswerError> {
    let text = one_line(text);
    match d.text_option {
        Some(t) if style.digits_select && t < 9 => {
            io.write((t + 1).to_string().as_bytes())?;
            io.wait(DIGIT_SETTLE).await?;
        }
        Some(t) => {
            go_to(io, d, NavRow::Option(t)).await?;
            io.wait(DIGIT_SETTLE).await?;
        }
        None if plain_prompt => {}
        None => {
            return Err(AnswerError::Invalid(AppError::new(
                "invalid_input",
                ONLY_OPTIONS,
            )))
        }
    }
    press(io, text.as_bytes()).await?;
    io.wait(DIGIT_SETTLE).await?;
    press(io, ENTER).await?;
    Ok(())
}

/// Send `answer` to the dialog `d` (as last parsed) and report whether the
/// screen changed. `plain_prompt`: a text prompt without options.
pub async fn answer<I: DialogIo + Send>(
    io: &mut I,
    d: &ScreenDialog,
    answer: &PromptAnswer,
    style: &DialogStyle,
    plain_prompt: bool,
) -> Result<Answered, AnswerError> {
    let before = io.dialog().unwrap_or_else(|| d.clone());
    match answer {
        PromptAnswer::Choice(i) if d.multi => choose_many(io, d, &[*i]).await?,
        PromptAnswer::Choice(i) => choose(io, d, *i, style).await?,
        PromptAnswer::Choices(list) => choose_many(io, d, list).await?,
        PromptAnswer::Text(t) => type_text(io, d, t, style, plain_prompt).await?,
    }
    let mut waited = Duration::ZERO;
    loop {
        let now = io.dialog();
        let same = now.as_ref().is_some_and(|n| {
            n.signature() == before.signature()
                && n.cursor == before.cursor
                && n.checked == before.checked
        });
        if !same {
            return Ok(Answered::Changed);
        }
        if waited >= CHANGE_WAIT {
            return Ok(Answered::Unchanged);
        }
        io.wait(POLL).await?;
        waited += POLL;
    }
}

#[cfg(test)]
#[path = "answer_tests.rs"]
mod tests;

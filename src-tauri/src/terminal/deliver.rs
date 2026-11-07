//! Delivering one message into Claude Code's input box, verified
//! (docs/ARCHITECTURE.md "Terminal mode", "Delivery").
//!
//! 1. [`gate`]: wait until the output was quiet for [`QUIET`] and the screen
//!    shows the empty input box after the last dialog ([`Screen::idle`]).
//!    While a dialog is on screen: one `notification` event per dialog, wait
//!    without timeout, never type. No box and no dialog: type after
//!    [`FALLBACK_QUIET`] of quiet.
//! 2. Type the text (a bracketed paste for several lines), wait
//!    [`TYPE_TO_ENTER`], press Enter (`\r`) separately.
//! 3. Wait up to [`SUBMIT_WAIT`] for the confirmation: the `submit` hook line
//!    for a prompt, the expected words on screen for a slash command.
//! 4. None: if the text still shows in the input box, press Enter again (up
//!    to [`ENTER_RETRIES`] times, [`ENTER_RETRY_WAIT`] apart); else (or after
//!    those) empty the box with Ctrl+U and start again from 1, up to the
//!    delivery's `cycles`.
//!
//! The steps run against a [`Port`] (the pty in the app, a simulated CLI in
//! the unit tests, `deliver_tests.rs`).

use std::future::Future;
use std::time::Duration;

use crate::contract::NodeEventKind;
use crate::terminal::live::Composer;
use crate::terminal::message::Typed;
use crate::terminal::screen::{normalize, Screen};

pub const QUIET: Duration = Duration::from_secs(1);
pub const FALLBACK_QUIET: Duration = Duration::from_secs(5);
pub const TYPE_TO_ENTER: Duration = Duration::from_millis(400);
pub const SUBMIT_WAIT: Duration = Duration::from_secs(4);
pub const ENTER_RETRY_WAIT: Duration = Duration::from_millis(1500);
pub const ENTER_RETRIES: u32 = 3;
pub const CLEAR_SETTLE: Duration = Duration::from_millis(300);
/// A dialog counts as drawn (and is announced) after this much quiet.
pub const DIALOG_SETTLE: Duration = Duration::from_millis(500);
pub const POLL: Duration = Duration::from_millis(200);
/// Full cycles for a prompt and for a slash command (`/model`).
pub const PROMPT_CYCLES: u32 = 3;
pub const SLASH_CYCLES: u32 = 2;
/// Ctrl+U: deletes the input box line up to the cursor (verified with the
/// real CLI 2.1.319; Esc only shows "Esc again to clear", a second Esc on an
/// empty box opens the rewind menu). One press per line, twice, plus two.
pub const CLEAR_KEY: u8 = 0x15;
/// Node failure reason after the last cycle.
pub const NOT_SUBMITTED: &str = "could not submit the message after 3 attempts; see the window";

/// Why delivery stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Halt {
    Killed,
    Exited(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Submitted,
    NotSubmitted,
}

/// What confirms that Claude Code took the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// The `submit` hook line (UserPromptSubmit).
    Submit,
    /// Any of these (lowercase) words in the output written since typing,
    /// not counting the echo of the typed text itself.
    Screen(Vec<String>),
    /// A CLI without a submit hook (Codex, custom): the typed text is gone
    /// from the input area and (a busy word showed since typing, or the
    /// empty-box placeholder is gone).
    Gone(Composer),
}

/// One message to deliver.
pub struct Delivery<'a> {
    pub typed: &'a Typed,
    pub expect: Expect,
    pub cycles: u32,
    /// Session window title, for the dialog notification.
    pub title: &'a str,
    /// Prefix of this delivery's node events ("" for a node's message).
    pub label: &'a str,
    /// No input box and no dialog on screen: type after this much quiet.
    pub ready_quiet: Duration,
}

impl<'a> Delivery<'a> {
    pub fn prompt(typed: &'a Typed, title: &'a str) -> Self {
        Self {
            typed,
            expect: Expect::Submit,
            cycles: PROMPT_CYCLES,
            title,
            label: "",
            ready_quiet: FALLBACK_QUIET,
        }
    }

    /// A prompt confirmed per `expect` (the harness's submit check).
    pub fn prompt_with(typed: &'a Typed, title: &'a str, expect: Expect, ready: Duration) -> Self {
        Self {
            expect,
            ready_quiet: ready,
            ..Self::prompt(typed, title)
        }
    }

    /// `/model <m>`: confirmed by "set model" or the model name on screen
    /// (a slash command fires no UserPromptSubmit hook). Events are prefixed
    /// with the command.
    pub fn model(typed: &'a Typed, model: &str, title: &'a str) -> Self {
        Self {
            typed,
            expect: Expect::Screen(vec!["set model".into(), model.to_lowercase()]),
            cycles: SLASH_CYCLES,
            title,
            label: typed.text(),
            ready_quiet: FALLBACK_QUIET,
        }
    }

    /// A step's node event text.
    fn step(&self, text: &str) -> String {
        if self.label.is_empty() {
            text.to_string()
        } else {
            format!("{}: {text}", self.label)
        }
    }
}

/// The notification text while a dialog waits for the user.
pub fn asking(title: &str) -> String {
    format!("Claude Code is asking something in the window {title}; answer it there")
}

/// What delivery needs from a session: keys, time, the screen, hooks, events.
pub trait Port {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Halt>;
    /// Sleep `d` unless the session is killed or exits first.
    fn wait(&mut self, d: Duration) -> impl Future<Output = Result<(), Halt>> + Send;
    /// Time since the last output.
    fn quiet_for(&self) -> Duration;
    fn screen(&self) -> Screen;
    /// Output position, for [`Port::since`].
    fn mark(&self) -> u64;
    /// The output written after `mark`, as a screen.
    fn since(&self, mark: u64) -> Screen;
    /// A `submit` hook line arrived since the last call.
    fn submitted(&mut self) -> bool;
    fn event(&mut self, kind: NodeEventKind, text: String);
    /// Output mark when the dialog on screen was announced (shared by every
    /// delivery of a session, so one dialog gets one notification).
    fn announced(&self) -> Option<u64>;
    fn set_announced(&mut self, mark: Option<u64>);
    /// The CLI's input area per `c` (from the live screen's rows).
    fn composer(&self, c: &Composer) -> Screen;
}

/// A dialog is on `screen`, not announced yet (no announcement, or dialog
/// markers drawn since the last one) and fully drawn (`quiet`).
pub fn dialog_due(
    screen: &Screen,
    quiet: Duration,
    announced: Option<u64>,
    since: impl FnOnce(u64) -> Screen,
) -> bool {
    screen.dialog_open()
        && quiet >= DIALOG_SETTLE
        && announced.map_or(true, |m| since(m).has_dialog())
}

/// Wait until Claude Code shows its empty input box (see the module doc).
pub async fn gate<P: Port + Send>(
    port: &mut P,
    title: &str,
    ready_quiet: Duration,
) -> Result<(), Halt> {
    loop {
        let screen = port.screen();
        let quiet = port.quiet_for();
        if screen.dialog_open() {
            if dialog_due(&screen, quiet, port.announced(), |m| port.since(m)) {
                port.event(NodeEventKind::Notification, asking(title));
                let mark = port.mark();
                port.set_announced(Some(mark));
            }
        } else {
            port.set_announced(None);
            if (quiet >= QUIET && screen.idle()) || quiet >= ready_quiet {
                return Ok(());
            }
        }
        port.wait(POLL).await?;
    }
}

/// Ctrl+U presses that empty a box holding `typed`.
pub fn clear_keys(typed: &Typed) -> Vec<u8> {
    let lines = typed.text().lines().count().max(1);
    vec![CLEAR_KEY; 2 * lines + 2]
}

/// Deliver `d` (see the module doc); node events tell each step.
pub async fn deliver<P: Port + Send>(port: &mut P, d: &Delivery<'_>) -> Result<Outcome, Halt> {
    let text = d.typed.text();
    for cycle in 1..=d.cycles {
        gate(port, d.title, d.ready_quiet).await?;
        let mark = port.mark();
        if cycle > 1 {
            port.write(&clear_keys(d.typed))?;
            port.wait(CLEAR_SETTLE).await?;
            port.event(
                NodeEventKind::Text,
                d.step(&format!("retyping (cycle {cycle})")),
            );
        }
        port.write(d.typed.payload().as_bytes())?;
        if cycle == 1 {
            port.event(NodeEventKind::Text, d.step("typed"));
        }
        port.wait(TYPE_TO_ENTER).await?;
        port.write(b"\r")?;
        if confirmed(port, d, mark, SUBMIT_WAIT).await? {
            return Ok(submitted(port, d));
        }
        for press in 2..=ENTER_RETRIES + 1 {
            if port.screen().dialog_open() || !still_typed(port, d, mark, text) {
                break;
            }
            port.event(
                NodeEventKind::Text,
                d.step(&format!("enter again ({press})")),
            );
            port.write(b"\r")?;
            if confirmed(port, d, mark, ENTER_RETRY_WAIT).await? {
                return Ok(submitted(port, d));
            }
        }
    }
    Ok(Outcome::NotSubmitted)
}

/// "submitted" (prompt) or "confirmed" (slash command on screen).
fn submitted<P: Port>(port: &mut P, d: &Delivery<'_>) -> Outcome {
    let word = match d.expect {
        Expect::Submit | Expect::Gone(_) => "submitted",
        Expect::Screen(_) => "confirmed",
    };
    port.event(NodeEventKind::Text, d.step(word));
    Outcome::Submitted
}

/// Wait up to `within` for the confirmation `d.expect`.
async fn confirmed<P: Port + Send>(
    port: &mut P,
    d: &Delivery<'_>,
    mark: u64,
    within: Duration,
) -> Result<bool, Halt> {
    let echo = normalize(d.typed.text());
    let mut waited = Duration::ZERO;
    loop {
        let ok = match &d.expect {
            Expect::Submit => port.submitted(),
            Expect::Screen(words) => {
                let shown = port.since(mark).text().replace(&echo, " ");
                words.iter().any(|w| shown.contains(w.as_str()))
            }
            Expect::Gone(c) => {
                let area = port.composer(c);
                let shown = port.since(mark).text().replace(&echo, " ");
                let busy = c.busy.iter().any(|w| shown.contains(w.as_str()));
                let placeholder = c.placeholders.iter().any(|p| area.contains(p));
                !area.shows_prefix(d.typed.text()) && (busy || !placeholder)
            }
        };
        if ok {
            return Ok(true);
        }
        if waited >= within {
            return Ok(false);
        }
        port.wait(POLL).await?;
        waited += POLL;
    }
}

/// The typed text still shows where it was typed (the input area for
/// [`Expect::Gone`], else after the last empty input box).
fn still_typed<P: Port>(port: &P, d: &Delivery<'_>, mark: u64, text: &str) -> bool {
    match &d.expect {
        Expect::Gone(c) => port.composer(c).shows_prefix(text),
        Expect::Submit | Expect::Screen(_) => port.since(mark).input_shows(text),
    }
}

#[cfg(test)]
#[path = "deliver_tests.rs"]
mod tests;

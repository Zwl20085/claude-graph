//! Unit tests of [`super::deliver`] against a simulated Claude Code: a
//! virtual clock, an output buffer drawn the way the real CLI draws its input
//! box (`❯` + no-break space), and switches for the failures seen in practice.

use std::time::Duration;

use super::*;
use crate::terminal::live::composer_text;

const BOX: &str = "\r\n\u{276f}\u{a0}";

#[derive(Default)]
struct Cli {
    now: Duration,
    out: String,
    out_at: Duration,
    line: String,
    pending: Option<String>,
    accepted: Vec<String>,
    submit_hook: bool,
    writes: Vec<String>,
    events: Vec<(NodeEventKind, String)>,
    announced: Option<u64>,
    // Behaviour switches.
    dialog_until: Duration,
    dialog_drawn: bool,
    swallow_first: bool,
    swallowed: bool,
    drop_enter: bool,
    deaf: bool,
    model_silent: bool,
    kill_at: Option<Duration>,
}

impl Cli {
    fn new() -> Self {
        let mut c = Self::default();
        c.print(&format!("Claude Code v2.1.319{BOX}  ? for shortcuts"));
        c
    }

    fn with_dialog(until: Duration) -> Self {
        let mut c = Self::new();
        c.dialog_until = until;
        c.print("\r\nNew MCP server found in this project \u{276f} Continue  Enter to confirm \u{b7} Esc to cancel");
        c.dialog_drawn = true;
        c
    }

    fn print(&mut self, s: &str) {
        self.out.push_str(s);
        self.out_at = self.now;
    }

    fn in_dialog(&self) -> bool {
        self.now < self.dialog_until
    }

    fn enter(&mut self) {
        let line = std::mem::take(&mut self.line);
        if self.in_dialog() || self.deaf {
            return;
        }
        if self.swallow_first && !self.swallowed {
            self.swallowed = true;
            self.print(BOX);
            return;
        }
        if self.drop_enter && !line.is_empty() {
            self.pending = Some(line.clone());
            self.print(&format!("{BOX}{line}"));
            return;
        }
        let text = self.pending.take().unwrap_or(line);
        if text.is_empty() {
            return;
        }
        if let Some(m) = text.strip_prefix("/model ") {
            if !self.model_silent {
                self.print(&format!("\r\n  \u{23bf} Set model to {m}{BOX}"));
            }
        } else {
            self.submit_hook = true;
            self.print(&format!("\r\n\u{25cf} working{BOX}"));
        }
        self.accepted.push(text);
    }
}

impl Port for Cli {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Halt> {
        let s = String::from_utf8_lossy(bytes).to_string();
        self.writes.push(s.clone());
        for c in s.chars() {
            match c {
                '\r' => self.enter(),
                '\u{15}' => self.line.clear(),
                c => {
                    self.line.push(c);
                    self.print(&c.to_string());
                }
            }
        }
        Ok(())
    }

    async fn wait(&mut self, d: Duration) -> Result<(), Halt> {
        self.now += d;
        if self.kill_at.is_some_and(|k| self.now >= k) {
            return Err(Halt::Killed);
        }
        if self.dialog_drawn && !self.in_dialog() {
            self.dialog_drawn = false;
            self.print(BOX);
        }
        Ok(())
    }

    fn quiet_for(&self) -> Duration {
        self.now - self.out_at
    }

    fn screen(&self) -> Screen {
        Screen::of(&self.out)
    }

    fn mark(&self) -> u64 {
        self.out.len() as u64
    }

    fn since(&self, mark: u64) -> Screen {
        Screen::of_all(&self.out[mark as usize..])
    }

    fn submitted(&mut self) -> bool {
        std::mem::take(&mut self.submit_hook)
    }

    fn event(&mut self, kind: NodeEventKind, text: String) {
        self.events.push((kind, text));
    }

    fn announced(&self) -> Option<u64> {
        self.announced
    }

    fn set_announced(&mut self, mark: Option<u64>) {
        self.announced = mark;
    }

    fn composer(&self, c: &Composer) -> Screen {
        let rows: Vec<String> = self
            .out
            .split('\n')
            .map(|l| l.trim_end_matches('\r').to_string())
            .collect();
        Screen::of_all(&composer_text(&rows, c))
    }
}

fn texts(cli: &Cli) -> Vec<&str> {
    cli.events.iter().map(|(_, t)| t.as_str()).collect()
}

fn run(cli: &mut Cli, d: &Delivery<'_>) -> Result<Outcome, Halt> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(deliver(cli, d))
}

const MSG: &str = "Reply with the single word ok and create no files";

#[test]
fn a_ready_cli_takes_the_message_at_once() {
    let mut cli = Cli::new();
    let typed = Typed::Line(MSG.into());
    let out = run(&mut cli, &Delivery::prompt(&typed, "T"));
    assert_eq!(out, Ok(Outcome::Submitted));
    assert_eq!(texts(&cli), ["typed", "submitted"]);
    assert_eq!(cli.accepted, [MSG]);
    // Text and Enter are separate writes, after one second of quiet.
    assert_eq!(cli.writes, [MSG, "\r"]);
    assert!(cli.now >= QUIET + TYPE_TO_ENTER);
}

#[test]
fn a_swallowed_message_is_cleared_and_retyped() {
    let mut cli = Cli::new();
    cli.swallow_first = true;
    let typed = Typed::Line(MSG.into());
    assert_eq!(
        run(&mut cli, &Delivery::prompt(&typed, "T")),
        Ok(Outcome::Submitted)
    );
    assert_eq!(texts(&cli), ["typed", "retyping (cycle 2)", "submitted"]);
    assert_eq!(cli.accepted, [MSG]);
    assert_eq!(cli.writes[2], "\u{15}".repeat(4));
}

#[test]
fn a_dropped_enter_is_pressed_again() {
    let mut cli = Cli::new();
    cli.drop_enter = true;
    let typed = Typed::Line(MSG.into());
    assert_eq!(
        run(&mut cli, &Delivery::prompt(&typed, "T")),
        Ok(Outcome::Submitted)
    );
    assert_eq!(texts(&cli), ["typed", "enter again (2)", "submitted"]);
    assert_eq!(cli.accepted, [MSG]);
}

#[test]
fn nothing_is_typed_while_a_dialog_is_up() {
    let mut cli = Cli::with_dialog(Duration::from_secs(30));
    let typed = Typed::Paste("line one\nline two".into());
    assert_eq!(
        run(&mut cli, &Delivery::prompt(&typed, "Win")),
        Ok(Outcome::Submitted)
    );
    assert_eq!(cli.events[0], (NodeEventKind::Notification, asking("Win")));
    assert_eq!(
        cli.events
            .iter()
            .filter(|(k, _)| *k == NodeEventKind::Notification)
            .count(),
        1
    );
    assert!(cli.now >= Duration::from_secs(31), "{:?}", cli.now);
    assert_eq!(cli.writes[0], typed.payload());
    assert_eq!(cli.accepted.len(), 1);
}

#[test]
fn a_kill_during_a_dialog_halts() {
    let mut cli = Cli::with_dialog(Duration::from_secs(3600));
    cli.kill_at = Some(Duration::from_secs(10));
    let typed = Typed::Line("x".into());
    assert_eq!(
        run(&mut cli, &Delivery::prompt(&typed, "T")),
        Err(Halt::Killed)
    );
    assert!(cli.writes.is_empty());
}

#[test]
fn a_deaf_cli_fails_after_three_cycles() {
    let mut cli = Cli::new();
    cli.deaf = true;
    let typed = Typed::Line(MSG.into());
    assert_eq!(
        run(&mut cli, &Delivery::prompt(&typed, "T")),
        Ok(Outcome::NotSubmitted)
    );
    let t = texts(&cli);
    assert_eq!(t.iter().filter(|e| e.starts_with("retyping")).count(), 2);
    assert!(t.contains(&"retyping (cycle 3)"));
    assert_eq!(t.iter().filter(|e| **e == "enter again (4)").count(), 3);
    assert!(!t.contains(&"submitted"));
    assert!(cli.accepted.is_empty());
}

#[test]
fn enter_is_retried_three_times_while_the_text_stays_in_the_box() {
    let mut cli = Cli::new();
    let typed = Typed::Line(MSG.into());
    // Text shows (echoed) but Enter never works.
    cli.deaf = true;
    let d = Delivery {
        cycles: 1,
        ..Delivery::prompt(&typed, "T")
    };
    assert_eq!(run(&mut cli, &d), Ok(Outcome::NotSubmitted));
    assert_eq!(
        texts(&cli),
        [
            "typed",
            "enter again (2)",
            "enter again (3)",
            "enter again (4)"
        ]
    );
    assert_eq!(cli.writes.iter().filter(|w| *w == "\r").count(), 4);
}

#[test]
fn model_is_confirmed_on_screen_without_a_hook() {
    let mut cli = Cli::new();
    let typed = Typed::Line("/model opus".into());
    let d = Delivery::model(&typed, "opus", "T");
    assert_eq!(run(&mut cli, &d), Ok(Outcome::Submitted));
    assert_eq!(cli.accepted, ["/model opus"]);

    let mut silent = Cli::new();
    silent.model_silent = true;
    assert_eq!(run(&mut silent, &d), Ok(Outcome::NotSubmitted));
    // The echo of "/model opus" alone does not confirm it.
    assert_eq!(silent.accepted, ["/model opus", "/model opus"]);
    assert!(texts(&silent).contains(&"/model opus: retyping (cycle 2)"));
    assert_eq!(
        texts(&cli),
        ["/model opus: typed", "/model opus: confirmed"]
    );
}

#[test]
fn clear_keys_cover_every_line() {
    assert_eq!(clear_keys(&Typed::Line("a".into())).len(), 4);
    assert_eq!(clear_keys(&Typed::Paste("a\nb\nc".into())).len(), 8);
}

fn gone() -> Expect {
    Expect::Gone(Composer {
        glyph: Some('\u{276f}'),
        rows: 2,
        placeholders: vec!["try".into()],
        busy: vec!["working".into()],
    })
}

#[test]
fn without_a_submit_hook_the_text_leaving_the_input_area_confirms() {
    let mut cli = Cli::new();
    let typed = Typed::Line(MSG.into());
    let d = Delivery::prompt_with(&typed, "T", gone(), Duration::from_secs(3));
    assert_eq!(run(&mut cli, &d), Ok(Outcome::Submitted));
    assert_eq!(texts(&cli), ["typed", "submitted"]);
    assert_eq!(cli.accepted, [MSG]);

    let mut dropped = Cli::new();
    dropped.drop_enter = true;
    assert_eq!(run(&mut dropped, &d), Ok(Outcome::Submitted));
    assert_eq!(texts(&dropped), ["typed", "enter again (2)", "submitted"]);

    let mut deaf = Cli::new();
    deaf.deaf = true;
    let once = Delivery { cycles: 1, ..d };
    assert_eq!(run(&mut deaf, &once), Ok(Outcome::NotSubmitted));
    assert!(texts(&deaf).contains(&"enter again (4)"));
}

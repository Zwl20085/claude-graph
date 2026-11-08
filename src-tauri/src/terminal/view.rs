//! The window of a session without a pty (a chained-headless harness such
//! as dsh): text the session renders is kept for `pty_replay` and emitted as
//! `pty:data`, like a pty's output; keys typed in the window are collected
//! into lines, echoed, and handed to the session as follow-up messages.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::contract::PtyData;
use crate::events::Emitter;
use crate::terminal::output::Output;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A rendered session window.
pub struct View {
    pub output: Arc<Output>,
    emitter: Arc<dyn Emitter>,
    label: String,
    run_id: String,
    sid: String,
    /// The line being typed in the window.
    line: Mutex<String>,
}

impl View {
    pub fn new(emitter: Arc<dyn Emitter>, label: &str, run_id: &str, sid: &str) -> Self {
        Self {
            output: Arc::new(Output::default()),
            emitter,
            label: label.to_string(),
            run_id: run_id.to_string(),
            sid: sid.to_string(),
            line: Mutex::new(String::new()),
        }
    }

    /// Show `text` (terminal text, `\r\n` line ends) in the window.
    pub fn show(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.output.touch();
        let seq = self.output.push(text);
        self.emitter.pty_data(
            &self.label,
            &PtyData {
                run_id: self.run_id.clone(),
                sid: self.sid.clone(),
                data: text.to_string(),
                seq,
            },
        );
    }

    /// Keys typed in the window: echoed; returns the lines completed by
    /// Enter (trimmed, non-empty). Escape sequences are dropped.
    pub fn keys(&self, text: &str) -> Vec<String> {
        let mut done = Vec::new();
        let mut echo = String::new();
        {
            let mut line = lock(&self.line);
            let mut chars = text.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '\x1b' => {
                        // ESC [ ... final byte, or ESC + one char.
                        if chars.next_if_eq(&'[').is_some() {
                            for n in chars.by_ref() {
                                if ('@'..='~').contains(&n) {
                                    break;
                                }
                            }
                        } else {
                            chars.next();
                        }
                    }
                    '\r' | '\n' => {
                        echo.push_str("\r\n");
                        let l = line.trim().to_string();
                        line.clear();
                        if !l.is_empty() {
                            done.push(l);
                        }
                    }
                    '\x7f' | '\x08' => {
                        if line.pop().is_some() {
                            echo.push_str("\x08 \x08");
                        }
                    }
                    c if !c.is_control() => {
                        line.push(c);
                        echo.push(c);
                    }
                    _ => {}
                }
            }
        }
        self.show(&echo);
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NullEmitter;

    #[test]
    fn keys_echo_and_complete_lines() {
        let v = View::new(Arc::new(NullEmitter), "term-r-s1", "r", "s1");
        assert!(v.keys("hel").is_empty());
        assert_eq!(v.keys("lo\x7f\x7fp\x1b[A me\r"), ["help me"]);
        assert_eq!(v.keys("  \r"), Vec::<String>::new());
        let shown = v.output.replay();
        assert!(shown.text.contains("hel") && shown.text.contains("\r\n"));
        v.show("x");
        assert_eq!(v.output.replay().seq, 4);
    }
}

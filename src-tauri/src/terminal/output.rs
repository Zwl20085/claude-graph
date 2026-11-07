//! The output of a session's process as the session reads it: the replay
//! buffer (`pty_replay`, marks for [`Output::since`]), the last output time,
//! the transcript length, the live screen grid and the CLI's markers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use crate::contract::PtyReplay;
use crate::terminal::live::LiveGrid;
use crate::terminal::pty::REPLAY_CAP;
use crate::terminal::screen::{Markers, Screen};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Output state shared by the reader thread and the session.
pub struct Output {
    /// Text of the emitted chunks (last [`REPLAY_CAP`] bytes), their count
    /// and the number of bytes emitted in all.
    replay: Mutex<(String, u64, u64)>,
    last: Mutex<Option<Instant>>,
    /// Bytes appended to `transcript.log` so far.
    pub(super) transcript: AtomicU64,
    /// The live screen as rows (for input-area checks).
    grid: Mutex<LiveGrid>,
    /// The CLI's dialog and input-box markers ([`Output::screen`]).
    markers: Mutex<Arc<Markers>>,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            replay: Mutex::default(),
            last: Mutex::default(),
            transcript: AtomicU64::default(),
            grid: Mutex::new(LiveGrid::default()),
            markers: Mutex::new(Markers::claude()),
        }
    }
}

impl Output {
    /// The emitted text so far (last [`REPLAY_CAP`] bytes) and its chunk count.
    pub fn replay(&self) -> PtyReplay {
        let buf = lock(&self.replay);
        PtyReplay {
            text: buf.0.clone(),
            seq: buf.1,
        }
    }

    /// When the process last printed anything.
    pub fn last_output(&self) -> Option<Instant> {
        *lock(&self.last)
    }

    /// What the end of the output shows now.
    pub fn screen(&self) -> Screen {
        let markers = self.markers();
        Screen::of(&lock(&self.replay).0).with(markers)
    }

    /// The output after `mark` as a screen judged with this CLI's markers.
    pub fn screen_since(&self, mark: u64) -> Screen {
        Screen::of_all(&self.since(mark)).with(self.markers())
    }

    pub fn markers(&self) -> Arc<Markers> {
        lock(&self.markers).clone()
    }

    pub fn set_markers(&self, markers: Arc<Markers>) {
        *lock(&self.markers) = markers;
    }

    /// The rows of the live screen.
    pub fn rows(&self) -> Vec<String> {
        lock(&self.grid).rows()
    }

    pub(super) fn resize_grid(&self, rows: u16, cols: u16) {
        lock(&self.grid).resize(rows, cols);
    }

    /// Bytes emitted so far, a mark for [`Output::since`].
    pub fn mark(&self) -> u64 {
        lock(&self.replay).2
    }

    /// Text emitted after `mark` (as much as the replay still holds).
    pub fn since(&self, mark: u64) -> String {
        let buf = lock(&self.replay);
        let new = usize::try_from(buf.2.saturating_sub(mark)).unwrap_or(usize::MAX);
        let mut start = buf.0.len().saturating_sub(new);
        while !buf.0.is_char_boundary(start) {
            start += 1;
        }
        buf.0[start..].to_string()
    }

    /// Length of `transcript.log` (bytes written by the reader so far).
    pub fn transcript_len(&self) -> u64 {
        self.transcript.load(Ordering::SeqCst)
    }

    /// Note output activity.
    pub(super) fn touch(&self) {
        *lock(&self.last) = Some(Instant::now());
    }

    /// Append one emitted chunk; returns its 1-based sequence number.
    pub(super) fn push(&self, text: &str) -> u64 {
        lock(&self.grid).feed(text);
        let mut buf = lock(&self.replay);
        buf.0.push_str(text);
        buf.2 += text.len() as u64;
        if buf.0.len() > REPLAY_CAP {
            let mut cut = buf.0.len() - REPLAY_CAP;
            while !buf.0.is_char_boundary(cut) {
                cut += 1;
            }
            buf.0.drain(..cut);
        }
        buf.1 += 1;
        buf.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_keeps_the_last_bytes_and_marks() {
        let out = Output::default();
        assert_eq!(out.push(&"a".repeat(REPLAY_CAP)), 1);
        assert_eq!(out.push("✻z"), 2);
        let replay = out.replay();
        assert!(replay.text.ends_with("✻z"));
        assert!(replay.text.len() <= REPLAY_CAP && replay.text.len() >= REPLAY_CAP - 3);
        assert_eq!(replay.seq, 2);
        assert!(out.last_output().is_none());
        out.touch();
        assert!(out.last_output().is_some());
        let mark = out.mark();
        assert_eq!(mark, (REPLAY_CAP + "✻z".len()) as u64);
        assert_eq!(out.since(mark), "");
        out.push("Do you trust this? Enter to confirm");
        assert_eq!(out.since(mark), "Do you trust this? Enter to confirm");
        assert!(out.screen().dialog_open());
        out.push("\r\n>\u{a0}");
        assert!(out.screen().idle());
        assert_eq!(out.since(0).len(), out.replay().text.len());
        assert!(out.rows().iter().any(|r| r.starts_with('>')));
        out.set_markers(Arc::new(Markers::of(&["ask codex"], &[])));
        assert!(!out.screen().idle());
    }
}

//! One Claude Code process in a pseudo terminal (portable-pty: ConPTY on
//! Windows, a pty on Unix), its reader thread and its transcript.
//!
//! The reader thread appends every chunk to `transcript.log`, keeps the last
//! [`REPLAY_CAP`] bytes in memory for `pty_replay`, emits `pty:data` to the
//! session window, and notices blocking dialogs ([`crate::terminal::screen::DIALOG_MARKERS`]). ConPTY
//! asks the terminal for the cursor position (`ESC[6n`) once at start and
//! waits for the answer; the reader answers it and drops it from the output,
//! so the process starts even with no window attached.
//!
//! A waiter thread blocks on the child's exit, then closes the pseudo console
//! (on Windows the reader only sees EOF after that). On Windows the child is
//! put in a kill-on-close Job Object; [`Pty::kill`] closes the job, kills the
//! child and runs `taskkill /T /F` (Unix: kills the process group).

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::watch;

use crate::contract::{AppError, AppResult, PtyData, PtyReplay};
use crate::events::Emitter;
use crate::job::Job;
use crate::terminal::screen::{count_dialogs, take_utf8};

/// Bytes of output kept in memory for `pty_replay`.
pub const REPLAY_CAP: usize = 512 * 1024;
const DSR: &[u8] = b"\x1b[6n";
const DSR_ANSWER: &[u8] = b"\x1b[1;1R";
const READ_BUF: usize = 16 * 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// What to start and where its output goes.
pub struct PtySpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Variables to set (`Some`) or remove (`None`) on top of the app's environment.
    pub env: Vec<(String, Option<String>)>,
    pub cols: u16,
    pub rows: u16,
    pub transcript: PathBuf,
    pub run_id: String,
    pub sid: String,
    /// Window label that receives `pty:data`.
    pub label: String,
}

/// Output state shared by the reader thread and the session.
#[derive(Default)]
pub struct Output {
    /// Text of the emitted chunks (last [`REPLAY_CAP`] bytes) and their count.
    replay: Mutex<(String, u64)>,
    last: Mutex<Option<Instant>>,
    /// Dialog markers seen so far.
    dialogs: AtomicU64,
    /// `dialogs` at the user's last keystroke.
    answered: AtomicU64,
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

    /// A dialog appeared after the user's last keystroke.
    pub fn dialog_open(&self) -> bool {
        self.dialogs.load(Ordering::SeqCst) > self.answered.load(Ordering::SeqCst)
    }

    fn user_typed(&self) {
        let seen = self.dialogs.load(Ordering::SeqCst);
        self.answered.store(seen, Ordering::SeqCst);
    }

    /// Note output activity.
    fn touch(&self) {
        *lock(&self.last) = Some(Instant::now());
    }

    /// Append one emitted chunk; returns its 1-based sequence number.
    fn push(&self, text: &str) -> u64 {
        let mut buf = lock(&self.replay);
        buf.0.push_str(text);
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

/// A live (or exited) pty process.
pub struct Pty {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    job: Arc<Mutex<Option<Job>>>,
    pid: Option<u32>,
    pub output: Arc<Output>,
    exit: watch::Receiver<Option<u32>>,
}

impl Pty {
    /// Spawn `spec` and start the reader and waiter threads.
    pub fn spawn(spec: PtySpec, emitter: Arc<dyn Emitter>) -> AppResult<Pty> {
        let fail = |what: &str, e: String| AppError::new("spawn_failed", format!("{what}: {e}"));
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: spec.rows,
                cols: spec.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| fail("could not open a pseudo terminal", e.to_string()))?;
        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        for (k, v) in &spec.env {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            }
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| {
            fail(
                &format!("could not start {}", spec.program.display()),
                e.to_string(),
            )
        })?;
        drop(pair.slave);
        let pid = child.process_id();
        #[cfg(windows)]
        let job = child.as_raw_handle().and_then(Job::assign_handle);
        #[cfg(not(windows))]
        let job: Option<Job> = None;
        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader();
        let writer = pair.master.take_writer();
        let (reader, writer) = match (reader, writer) {
            (Ok(r), Ok(w)) => (r, w),
            (Err(e), _) | (_, Err(e)) => {
                let _ = child.kill();
                return Err(fail("pty streams", e.to_string()));
            }
        };
        let master = Arc::new(Mutex::new(Some(pair.master)));
        let job = Arc::new(Mutex::new(job));
        let output = Arc::new(Output::default());
        let (exit_tx, exit) = watch::channel(None);
        let pty = Pty {
            writer: Arc::new(Mutex::new(writer)),
            master: master.clone(),
            killer: Mutex::new(killer),
            job: job.clone(),
            pid,
            output: output.clone(),
            exit,
        };
        let answer = pty_answerer(&pty);
        std::thread::spawn(move || read_loop(reader, spec, output, emitter, answer));
        std::thread::spawn(move || {
            let code = match child.wait() {
                Ok(status) => status.exit_code(),
                Err(e) => {
                    log::warn!("waiting for the session process failed: {e}");
                    u32::MAX
                }
            };
            // Closing the pseudo console lets the reader reach EOF (ConPTY).
            drop(lock(&master).take());
            drop(lock(&job).take());
            let _ = exit_tx.send(Some(code));
        });
        Ok(pty)
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Exit code once the process has exited.
    pub fn exit_code(&self) -> Option<u32> {
        *self.exit.borrow()
    }

    /// A receiver that changes to `Some(code)` when the process exits.
    pub fn exit_watch(&self) -> watch::Receiver<Option<u32>> {
        self.exit.clone()
    }

    /// Write raw bytes. `user`: typed in the window (answers an open dialog).
    pub fn write(&self, bytes: &[u8], user: bool) -> AppResult<()> {
        if self.exit_code().is_some() {
            return Err(AppError::new(
                "not_found",
                "the Claude Code process has exited",
            ));
        }
        let mut w = lock(&self.writer);
        w.write_all(bytes)
            .and_then(|_| w.flush())
            .map_err(|e| AppError::new("io", format!("writing to the terminal failed: {e}")))?;
        if user {
            self.output.user_typed();
        }
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> AppResult<()> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        match lock(&self.master).as_ref() {
            Some(m) => m
                .resize(size)
                .map_err(|e| AppError::new("io", format!("resize failed: {e}"))),
            None => Err(AppError::new(
                "not_found",
                "the Claude Code process has exited",
            )),
        }
    }

    /// Kill the process tree and wait (briefly) for the exit.
    pub async fn kill(&self) {
        drop(lock(&self.job).take());
        if let Err(e) = lock(&self.killer).kill() {
            log::debug!("killing the session process: {e}");
        }
        if let Some(pid) = self.pid {
            crate::runner::kill_tree(pid).await;
        }
        let mut exit = self.exit.clone();
        let waited = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            exit.wait_for(Option::is_some),
        )
        .await;
        if waited.is_err() {
            log::warn!("session process {:?} did not exit after kill", self.pid);
            drop(lock(&self.master).take());
        }
    }
}

type Answer = Box<dyn Fn(&[u8]) + Send>;

/// Writes the reader's terminal answers (the cursor report) to the pty.
fn pty_answerer(pty: &Pty) -> Answer {
    let writer = pty.writer.clone();
    Box::new(move |bytes| {
        let mut w = lock(&writer);
        if let Err(e) = w.write_all(bytes).and_then(|_| w.flush()) {
            log::warn!("answering the terminal query failed: {e}");
        }
    })
}

fn read_loop(
    mut reader: Box<dyn Read + Send>,
    spec: PtySpec,
    output: Arc<Output>,
    emitter: Arc<dyn Emitter>,
    answer: Answer,
) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&spec.transcript)
        .map_err(|e| log::warn!("transcript {}: {e}", spec.transcript.display()))
        .ok();
    let mut buf = vec![0u8; READ_BUF];
    let mut pending: Vec<u8> = Vec::new();
    let mut answered_dsr = false;
    let mut scan_tail = String::new();
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                log::debug!("session {} reader ended: {e}", spec.sid);
                break;
            }
        };
        let mut chunk = buf[..n].to_vec();
        if !answered_dsr {
            if let Some(i) = chunk.windows(DSR.len()).position(|w| w == DSR) {
                answered_dsr = true;
                chunk.drain(i..i + DSR.len());
                answer(DSR_ANSWER);
            }
        }
        if chunk.is_empty() {
            continue;
        }
        if let Some(f) = file.as_mut() {
            if let Err(e) = f.write_all(&chunk) {
                log::warn!("appending to the transcript failed: {e}");
                file = None;
            }
        }
        output.touch();
        let text = take_utf8(&mut pending, &chunk);
        let found = count_dialogs(&mut scan_tail, &text);
        if found > 0 {
            output.dialogs.fetch_add(found, Ordering::SeqCst);
        }
        if !text.is_empty() {
            // Counted into the replay before it is emitted: a window that
            // replayed seq N drops live chunks up to N.
            let seq = output.push(&text);
            let data = PtyData {
                run_id: spec.run_id.clone(),
                sid: spec.sid.clone(),
                data: text,
                seq,
            };
            emitter.pty_data(&spec.label, &data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_keeps_the_last_bytes_and_tracks_dialogs() {
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
        assert!(!out.dialog_open());
        out.dialogs.fetch_add(1, Ordering::SeqCst);
        assert!(out.dialog_open());
        out.user_typed();
        assert!(!out.dialog_open());
    }
}

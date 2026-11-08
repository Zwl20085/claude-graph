//! One harness CLI process (Claude Code, Codex, …) in a pseudo terminal (portable-pty: ConPTY on
//! Windows, a pty on Unix), its reader thread and its transcript.
//!
//! The reader thread appends every chunk to `transcript.log`, keeps the last
//! [`REPLAY_CAP`] bytes in memory for `pty_replay`, emits `pty:data` to the
//! session window, and counts the bytes emitted (marks for [`Output::since`]). ConPTY
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
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::watch;

use crate::contract::{AppError, AppResult, PtyData};
use crate::events::Emitter;
use crate::job::Job;
use crate::terminal::emulate::TermSize;
use crate::terminal::screen::{take_utf8, Markers};

pub use crate::terminal::output::Output;

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
    /// The CLI's dialog and input-box markers.
    pub markers: Arc<Markers>,
}

/// A live (or exited) pty process.
pub struct Pty {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    job: Arc<Mutex<Option<Job>>>,
    pid: Option<u32>,
    /// Every size set (at spawn and by [`Pty::resize`]) with the transcript
    /// length at that moment.
    sizes: Mutex<Vec<TermSize>>,
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
        if crate::claude_bin::is_batch_file(&spec.program) {
            if let Err(c) = crate::claude_bin::batch_args_safe(&spec.args) {
                return Err(AppError::new(
                    "spawn_failed",
                    format!(
                        "{} is a batch file and an argument holds {c:?}, which cmd.exe would \
                         interpret; point the harness at the program's .exe (or node and its \
                         script) instead of the .cmd/.bat shim",
                        spec.program.display()
                    ),
                ));
            }
        }
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
        output.set_markers(spec.markers.clone());
        output.resize_grid(spec.rows, spec.cols);
        let (exit_tx, exit) = watch::channel(None);
        let pty = Pty {
            writer: Arc::new(Mutex::new(writer)),
            master: master.clone(),
            killer: Mutex::new(killer),
            job: job.clone(),
            pid,
            sizes: Mutex::new(vec![TermSize {
                at: 0,
                cols: spec.cols,
                rows: spec.rows,
            }]),
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

    /// Every terminal size set so far, with the transcript offset it took
    /// effect at (for replaying the transcript, [`crate::terminal::emulate`]).
    pub fn sizes(&self) -> Vec<TermSize> {
        lock(&self.sizes).clone()
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

    /// Write raw bytes (keys) to the process.
    pub fn write(&self, bytes: &[u8]) -> AppResult<()> {
        if self.exit_code().is_some() {
            return Err(AppError::new(
                "not_found",
                "the Claude Code process has exited",
            ));
        }
        let mut w = lock(&self.writer);
        w.write_all(bytes)
            .and_then(|_| w.flush())
            .map_err(|e| AppError::new("io", format!("writing to the terminal failed: {e}")))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> AppResult<()> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        match lock(&self.master).as_ref() {
            Some(m) => {
                m.resize(size)
                    .map_err(|e| AppError::new("io", format!("resize failed: {e}")))?;
                self.output.resize_grid(rows, cols);
                let at = self.output.transcript_len();
                let mut sizes = lock(&self.sizes);
                if !sizes
                    .last()
                    .is_some_and(|s| (s.cols, s.rows) == (cols, rows))
                {
                    sizes.push(TermSize { at, cols, rows });
                }
                Ok(())
            }
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
            match f.write_all(&chunk) {
                Ok(()) => {
                    output
                        .transcript
                        .fetch_add(chunk.len() as u64, Ordering::SeqCst);
                }
                Err(e) => {
                    log::warn!("appending to the transcript failed: {e}");
                    file = None;
                }
            }
        }
        output.touch();
        let text = take_utf8(&mut pending, &chunk);
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

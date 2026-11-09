//! The tips folder on disk (docs/AGENT-PROTOCOL.md section 7): listing, the
//! bounded frontmatter read, the file stamp, and the whole-file write with
//! stamp check and retries.
//!
//! A write reads the file and its stamp (`<mtime ns>-<byte length>`), lets
//! the caller compute the new text from exactly that content, writes a temp
//! file `.<name>.<pid>-<n>.tmp` in the same folder (written and flushed to
//! disk), checks right before the rename that the target still exists with
//! the same stamp, and renames over it. A changed stamp means re-read and
//! re-apply (up to [`STAMP_TRIES`] times). Windows sharing violations
//! (OneDrive or the board holding the file) are retried with backoff; a
//! read-only tip is reported as such, never retried forever. A file that
//! vanished is never recreated.
//!
//! Remaining window: between that last check and the rename (microseconds),
//! an edit by the board or a delete would be lost or undone; the OS gives no
//! compare-and-swap rename. OneDrive conflict copies and the next scan's hash
//! check catch the edit case (docs/INBOX.md "Known limitations").

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

pub const STAMP_TRIES: usize = 5;
/// Most bytes read to find the end of a tip's frontmatter.
pub const FRONTMATTER_CAP: usize = 64 * 1024;
/// Read buffer of the frontmatter read: at most this much past the closing
/// fence is fetched from the OS, and it is never decoded or looked at.
const PREFIX_BUF: usize = 256;
/// Backoff of the sharing-violation retries.
const SHARING_BACKOFF_MS: [u64; 6] = [50, 100, 200, 400, 800, 1600];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A tip file of the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub path: PathBuf,
}

/// `*.md` directly in `folder`, not starting with `.` or `~`, regular files
/// only (symbolic links are skipped), sorted by name.
pub fn list(folder: &Path) -> io::Result<Vec<Entry>> {
    let mut out: Vec<Entry> = fs::read_dir(folder)?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let (stem, ext) = name.rsplit_once('.')?;
            let ok =
                ext.eq_ignore_ascii_case("md") && !stem.is_empty() && !name.starts_with(['.', '~']);
            ok.then(|| Entry {
                id: stem.to_string(),
                path: e.path(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// The start of a tip, up to and including its closing fence line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prefix {
    /// BOM, fences and frontmatter (nothing of the body).
    Frontmatter(String),
    /// The first line is not a `---` fence, or the file ends with no closing fence.
    NoFrontmatter,
    /// No closing fence within [`FRONTMATTER_CAP`] bytes.
    TooLong,
    NotUtf8,
}

fn fence_line(line: &[u8]) -> bool {
    matches!(line, b"---\n" | b"---\r\n" | b"---")
}

/// Read line by line until the closing fence (protocol section 9: a tip that
/// is not an agent tip is not read past its frontmatter).
pub fn read_frontmatter(path: &Path) -> io::Result<Prefix> {
    with_sharing_retry(|| {
        let mut reader = BufReader::with_capacity(PREFIX_BUF, File::open(path)?);
        let mut buf: Vec<u8> = Vec::new();
        let mut line: Vec<u8> = Vec::new();
        let mut first = true;
        loop {
            line.clear();
            let room = (FRONTMATTER_CAP + 1).saturating_sub(buf.len()) as u64;
            let n = (&mut reader).take(room).read_until(b'\n', &mut line)?;
            if n == 0 {
                return Ok(Prefix::NoFrontmatter);
            }
            let content: &[u8] = if first {
                line.strip_prefix("\u{feff}".as_bytes()).unwrap_or(&line)
            } else {
                &line
            };
            let is_fence = fence_line(content);
            let at_eof = !line.ends_with(b"\n");
            buf.extend_from_slice(&line);
            if buf.len() > FRONTMATTER_CAP {
                return Ok(Prefix::TooLong);
            }
            if first {
                if !is_fence || at_eof {
                    return Ok(Prefix::NoFrontmatter);
                }
                first = false;
            } else if is_fence {
                return Ok(String::from_utf8(buf).map_or(Prefix::NotUtf8, Prefix::Frontmatter));
            } else if at_eof {
                return Ok(Prefix::NoFrontmatter);
            }
        }
    })
}

/// `<mtime ns>-<byte length>`.
pub fn stamp(path: &Path) -> io::Result<String> {
    let meta = fs::metadata(path)?;
    let ns = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    Ok(format!("{ns}-{}", meta.len()))
}

fn is_read_only(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.permissions().readonly())
}

/// Windows: sharing / lock violation, or access denied while a replace is in flight.
fn is_sharing(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// `op` retried on sharing violations with backoff.
pub fn with_sharing_retry<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    for wait in SHARING_BACKOFF_MS {
        match op() {
            Err(e) if is_sharing(&e) => std::thread::sleep(Duration::from_millis(wait)),
            other => return other,
        }
    }
    op()
}

/// Bytes and stamp, read with sharing retries.
pub fn read(path: &Path) -> io::Result<(Vec<u8>, String)> {
    with_sharing_retry(|| {
        let before = stamp(path)?;
        let bytes = fs::read(path)?;
        Ok((bytes, before))
    })
}

/// Why a whole-file write did not happen.
#[derive(Debug)]
pub enum WriteError<E> {
    /// The file is gone (deleted or renamed): nothing is written or recreated.
    Gone,
    /// Not UTF-8 text.
    NotUtf8,
    /// The caller refused the content.
    Refused(E),
    /// The stamp changed on every try.
    Conflict,
    /// The tip file has the read-only attribute.
    ReadOnly,
    Io(io::Error),
}

impl<E: std::fmt::Debug> WriteError<E> {
    pub fn describe(&self, refused: impl Fn(&E) -> String) -> String {
        match self {
            WriteError::Gone => "the tip file is gone; nothing written".into(),
            WriteError::NotUtf8 => "not UTF-8 text; nothing written".into(),
            WriteError::Refused(e) => refused(e),
            WriteError::Conflict => {
                format!("the tip kept changing ({STAMP_TRIES} tries); nothing written")
            }
            WriteError::ReadOnly => "tip file is read-only; nothing written".into(),
            WriteError::Io(e) => format!("i/o error: {e}"),
        }
    }
}

fn gone_or<E>(e: io::Error) -> WriteError<E> {
    if e.kind() == io::ErrorKind::NotFound {
        WriteError::Gone
    } else {
        WriteError::Io(e)
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}-{n}.tmp", std::process::id()))
}

fn remove_temp(temp: &Path) {
    if let Err(e) = fs::remove_file(temp) {
        if e.kind() != io::ErrorKind::NotFound {
            log::warn!("inbox: could not remove {}: {e}", temp.display());
        }
    }
}

/// The temp file, written and flushed to disk before it is renamed.
fn write_temp(temp: &Path, bytes: &[u8]) -> io::Result<()> {
    with_sharing_retry(|| {
        let mut f = File::create(temp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })
}

/// Test hook: runs between the temp write and the final stamp check.
pub type BeforeRename<'a> = &'a dyn Fn(usize);

/// Read `path`, compute the new text with `apply` from exactly what was
/// read, and replace the file if its stamp did not change meanwhile.
pub fn update<T, E: std::fmt::Debug>(
    path: &Path,
    apply: impl Fn(&str) -> Result<(String, T), E>,
) -> Result<T, WriteError<E>> {
    update_with(path, apply, &|_| {})
}

/// [`update`] with a hook between the temp write and the stamp check (tests).
pub fn update_with<T, E: std::fmt::Debug>(
    path: &Path,
    apply: impl Fn(&str) -> Result<(String, T), E>,
    before_rename: BeforeRename<'_>,
) -> Result<T, WriteError<E>> {
    for attempt in 0..STAMP_TRIES {
        let (bytes, before) = read(path).map_err(gone_or)?;
        let text = String::from_utf8(bytes).map_err(|_| WriteError::NotUtf8)?;
        let (new_text, value) = apply(&text).map_err(WriteError::Refused)?;
        if is_read_only(path) {
            return Err(WriteError::ReadOnly);
        }
        let temp = temp_path(path);
        if let Err(e) = write_temp(&temp, new_text.as_bytes()) {
            remove_temp(&temp);
            return Err(WriteError::Io(e));
        }
        before_rename(attempt);
        // Last check, right before the rename: the target still exists
        // (never recreated) and nobody wrote it since it was read.
        match stamp(path) {
            Ok(now) if now == before => {}
            Ok(_) => {
                remove_temp(&temp);
                continue;
            }
            Err(e) => {
                remove_temp(&temp);
                return Err(gone_or(e));
            }
        }
        if is_read_only(path) {
            remove_temp(&temp);
            return Err(WriteError::ReadOnly);
        }
        return match with_sharing_retry(|| fs::rename(&temp, path)) {
            Ok(()) => Ok(value),
            Err(e) => {
                remove_temp(&temp);
                Err(if is_read_only(path) {
                    WriteError::ReadOnly
                } else {
                    gone_or(e)
                })
            }
        };
    }
    Err(WriteError::Conflict)
}

#[cfg(test)]
#[path = "folder_tests.rs"]
mod tests;

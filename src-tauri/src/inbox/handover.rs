//! Handing a round's results over to the tips folder: the copy into
//! `agent/<tip id>/` (with `result.md` rewritten by [`relink`]), every text
//! written into the tip rewritten the same way (the agent's reply or
//! question, and also dialog lines and failure reasons, which can quote the
//! agent's tool calls or OS errors with local paths), and the names of what
//! was not copied (shown on the tip, [`super::block::not_copied_line`]).

use std::path::{Path, PathBuf};

use crate::paths::abs;
use crate::run_store::{node_run_dir, RESULT_FILE};
use crate::store::deliverables_dir;

use super::block::{Footnotes, Outcome};
use super::graphgen::NODE_ID;
use super::relink::{relink, CopiedFile, Places};
use super::results::{copy_round, results_dir, CopyReport, REPLY_NAME, RESULTS_FOLDER_NAME};
use super::runner::Inbox;
use super::watch::Round;

/// Most of a text that is rewritten for the tip (the block keeps at most
/// 4000 chars of it anyway; the cut and its pointer still apply).
pub const TIP_TEXT_MAX: usize = 64 * 1024;

/// The Windows 8.3 form of an existing `p`, when it has one.
#[cfg(windows)]
fn short_name(p: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    let len = u32::try_from(buf.len()).ok()?;
    // SAFETY: `wide` is NUL-terminated and `buf` holds `len` u16s; the call
    // writes at most that many and returns the length written (0: failure).
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), len) };
    let n = usize::try_from(n).ok()?;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])))
}

#[cfg(not(windows))]
fn short_name(_p: &Path) -> Option<PathBuf> {
    None
}

/// `p` as given, resolved and in 8.3 form (the agent may see any).
fn spellings(p: &Path) -> Vec<PathBuf> {
    let resolved = abs(p);
    let mut v = vec![p.to_path_buf(), resolved.clone()];
    v.extend(short_name(&resolved));
    v.dedup();
    v
}

/// `%VAR%` spellings of `paths` under the folder in environment `var`.
fn env_spellings(paths: &[PathBuf], var: &str) -> Vec<PathBuf> {
    let Some(base) = std::env::var_os(var).filter(|v| !v.is_empty()) else {
        return Vec::new();
    };
    let base = PathBuf::from(base);
    paths
        .iter()
        .filter_map(|p| p.strip_prefix(&base).ok())
        .map(|rest| Path::new(&format!("%{var}%")).join(rest))
        .collect()
}

fn with_env(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let extra = env_spellings(&paths, "APPDATA");
    paths.into_iter().chain(extra).collect()
}

/// Where the round's files are, for [`relink`].
pub(crate) fn places(inbox: &Inbox, round: &Round, files: &[CopiedFile]) -> Places {
    let data = inbox.env().app.data_dir();
    let home: Vec<PathBuf> = dirs::home_dir()
        .map(|h| spellings(&h))
        .unwrap_or_default()
        .into_iter()
        .chain([PathBuf::from("%USERPROFILE%")])
        .collect();
    Places {
        tip_id: round.tip_id.clone(),
        deliverables: with_env(spellings(&deliverables_dir(&round.graph_dir, NODE_ID))),
        private: with_env([spellings(&round.graph_dir), spellings(data)].concat()),
        tips: spellings(&round.folder),
        home,
        files: files.to_vec(),
    }
}

/// The reply and the deliverables to `<tips>/agent/<id>/` (protocol 7.3),
/// `result.md` with its local paths rewritten. Refusals are logged.
pub(crate) fn copy_results(inbox: &Inbox, round: &Round) -> CopyReport {
    let id = &round.tip_id;
    let dest = match results_dir(&round.folder, id) {
        Ok(dest) => dest,
        Err(e) => {
            inbox.log(Some(id), format!("results folder refused: {e}"));
            return CopyReport {
                not_copied: vec![RESULTS_FOLDER_NAME.to_string()],
                ..CopyReport::default()
            };
        }
    };
    let result_md = round
        .graph_run
        .as_deref()
        .map(|r| node_run_dir(&round.graph_dir, r, NODE_ID).join(RESULT_FILE));
    let fix = |text: &str, report: &CopyReport| relink(text, &places(inbox, round, &report.files));
    let report = copy_round(
        &dest,
        result_md.as_deref(),
        round.prompt.as_deref(),
        &deliverables_dir(&round.graph_dir, NODE_ID),
        &fix,
    );
    for r in &report.refused {
        inbox.log(Some(id), format!("not copied: {r}"));
    }
    report
}

/// At most [`TIP_TEXT_MAX`] bytes of `text`, cut at a char boundary.
fn capped(text: &str) -> &str {
    let mut end = text.len().min(TIP_TEXT_MAX);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The outcome as it goes into the tip: every text in it with its local
/// paths rewritten ([`relink`]).
pub(crate) fn for_tip(outcome: &Outcome, p: &Places) -> Outcome {
    let fix = |t: &str| relink(capped(t), p);
    match outcome {
        Outcome::Done { reply } => Outcome::Done { reply: fix(reply) },
        Outcome::Question { text } => Outcome::Question { text: fix(text) },
        Outcome::Dialog { text } => Outcome::Dialog { text: fix(text) },
        Outcome::Failed { reason } => Outcome::Failed {
            reason: fix(reason),
        },
    }
}

/// The names the tip lists as not copied: what the copy refused, and the
/// reply of a done round when it left none (a failed round may have none).
pub(crate) fn not_copied(outcome: &Outcome, report: &CopyReport) -> Vec<String> {
    let mut names = report.not_copied.clone();
    let done = matches!(outcome, Outcome::Done { .. });
    if done && report.reply_missing && !names.iter().any(|n| n == REPLY_NAME) {
        names.push(REPLY_NAME.to_string());
    }
    names
}

/// The runner's footnotes for the block ([`Footnotes`]).
pub(crate) fn footnotes(outcome: &Outcome, report: &CopyReport) -> Footnotes {
    Footnotes {
        not_copied: not_copied(outcome, report),
        reply_as_written: report.reply_as_written.map(str::to_string),
    }
}

#[cfg(test)]
#[path = "handover_tests.rs"]
mod tests;

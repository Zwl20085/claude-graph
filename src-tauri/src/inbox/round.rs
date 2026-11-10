//! The folder a round runs in, made safe to start a session in.
//!
//! Claude Code loads `CLAUDE.md` from every parent of its working folder and
//! project settings from a git root, so the planted-files check
//! ([`super::start::planted_files`]) must see the folders the session will
//! really run in. The path is built from the canonical data dir, and every
//! level the Inbox owns (`inbox`, `inbox/rounds`, `inbox/rounds/<tip>`, the
//! round folder) must be a plain directory, never a link or junction (an
//! agent with a shell could otherwise swap one for a junction to a place
//! whose parents hold such files). The round folder is created with
//! `create_dir` (a round folder is always new), must canonicalize to itself,
//! and everything is checked again right before the run starts.
//!
//! Each check takes `data`, the canonical app data dir.

use std::io;
use std::path::{Path, PathBuf};

use super::fsguard::require_plain_dir;
use super::graphgen::{inbox_root, round_dir, ROUNDS_DIR};
use super::start::planted_files;

/// The levels the Inbox owns above a round folder, top first.
fn owned_levels(data: &Path, round: &Path) -> Result<Vec<PathBuf>, String> {
    let inbox = inbox_root(data);
    let rounds = inbox.join(ROUNDS_DIR);
    let tip = round
        .parent()
        .filter(|t| t.parent() == Some(rounds.as_path()))
        .ok_or_else(|| {
            format!(
                "{} is outside the Inbox rounds folder {}",
                round.display(),
                rounds.display()
            )
        })?;
    Ok(vec![inbox, rounds, tip.to_path_buf()])
}

/// Each existing owned level is a plain directory (missing ones are fine).
fn existing_levels_plain(levels: &[PathBuf]) -> Result<(), String> {
    levels
        .iter()
        .filter(|l| std::fs::symlink_metadata(l).is_ok())
        .try_for_each(|l| require_plain_dir(l))
}

fn planted(dir: &Path, data: &Path) -> Result<(), String> {
    match planted_files(dir, data) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// Before the claim (nothing is created): the owned levels that exist are
/// plain directories, the round folder does not exist yet, and no planted
/// file is in it or above it. Returns the round folder's path.
pub fn check_before_claim(data: &Path, tip_id: &str, run: &str) -> Result<PathBuf, String> {
    let dir = round_dir(&inbox_root(data), tip_id, run);
    existing_levels_plain(&owned_levels(data, &dir)?)?;
    if std::fs::symlink_metadata(&dir).is_ok() {
        return Err(format!("the round folder {} already exists", dir.display()));
    }
    planted(&dir, data)?;
    Ok(dir)
}

/// After the claim: the owned levels are made (each as a plain directory),
/// then the round folder itself with `create_dir` (refused when it exists);
/// it must canonicalize to itself and carry no planted file above it.
pub fn create(data: &Path, tip_id: &str, run: &str) -> Result<PathBuf, String> {
    let dir = round_dir(&inbox_root(data), tip_id, run);
    for level in owned_levels(data, &dir)? {
        match std::fs::create_dir(&level) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", level.display())),
        }
        require_plain_dir(&level)?;
    }
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return Err(format!("the round folder {} already exists", dir.display()))
        }
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    recheck(data, &dir)?;
    Ok(dir)
}

/// The last check before the run starts: every owned level and the round
/// folder are plain directories, the folder canonicalizes to itself (no
/// level resolves elsewhere), and no planted file is in it or above it.
pub fn recheck(data: &Path, dir: &Path) -> Result<(), String> {
    if !dir.starts_with(data) {
        return Err(format!(
            "{} is outside the app data folder {}",
            dir.display(),
            data.display()
        ));
    }
    for level in owned_levels(data, dir)? {
        require_plain_dir(&level)?;
    }
    require_plain_dir(dir)?;
    let canon = crate::paths::abs(dir);
    if canon != dir {
        return Err(format!(
            "the round folder {} resolves to {}",
            dir.display(),
            canon.display()
        ));
    }
    planted(dir, data)
}

#[cfg(test)]
#[path = "round_tests.rs"]
mod tests;

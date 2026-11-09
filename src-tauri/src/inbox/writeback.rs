//! Claim and finish on the tip file: [`super::ops`] applied through
//! [`super::folder::update_gated`], so a concurrent edit is re-read and the
//! patch re-applied, and a vanished file is reported and never recreated.
//! Before every whole-file read, `kind` is checked from the bounded
//! frontmatter read (protocol section 9: a tip that is not an agent tip is
//! not read past its frontmatter).

use std::path::Path;

use super::block::Outcome;
use super::folder::{gone_or, read_frontmatter, update_gated, Prefix, WriteError, FRONTMATTER_CAP};
use super::ops::{self, Claimant, Claimed, Finished, Known, OpError};
use super::tip::{parse, peek_kind, TipError, KIND_AGENT};

/// Why a write-back was dropped.
#[derive(Debug)]
pub struct Dropped {
    pub error: WriteError<OpError>,
}

impl Dropped {
    pub fn describe(&self) -> String {
        self.error.describe(OpError::describe)
    }

    pub fn is_gone(&self) -> bool {
        matches!(self.error, WriteError::Gone)
    }

    /// The file may take the write later (read-only, locked, kept changing):
    /// a finished outcome is kept and retried at the next scan.
    pub fn is_retryable(&self) -> bool {
        match &self.error {
            WriteError::ReadOnly | WriteError::Conflict | WriteError::Io(_) => true,
            WriteError::Gone | WriteError::NotUtf8 | WriteError::Refused(_) => false,
        }
    }

    /// The tip is gone, no longer an agent tip, or claimed by another host
    /// or run: this round must not touch it or its results any more.
    pub fn is_lost(&self) -> bool {
        matches!(
            self.error,
            WriteError::Gone | WriteError::Refused(OpError::NotAgent | OpError::NotOurs(_))
        )
    }
}

/// The frontmatter of `path` (bounded read), or why it has none.
fn frontmatter_text(path: &Path) -> Result<String, WriteError<OpError>> {
    let unreadable = |why: String| WriteError::Refused(OpError::Unreadable(why));
    match read_frontmatter(path).map_err(gone_or)? {
        Prefix::Frontmatter(text) => Ok(text),
        Prefix::NotUtf8 => Err(WriteError::NotUtf8),
        Prefix::NoFrontmatter => Err(unreadable(TipError::NoFrontmatter.describe())),
        Prefix::TooLong => Err(unreadable(format!(
            "no closing --- within the first {} KiB",
            FRONTMATTER_CAP / 1024
        ))),
    }
}

/// `kind: agent`, judged from the bounded frontmatter read only.
fn agent_gate(path: &Path) -> Result<(), WriteError<OpError>> {
    let text = frontmatter_text(path)?;
    match peek_kind(text.as_bytes()) {
        Ok(Some(kind)) if kind == KIND_AGENT => Ok(()),
        Ok(_) => Err(WriteError::Refused(OpError::NotAgent)),
        Err(e) => Err(WriteError::Refused(OpError::Unreadable(e.describe()))),
    }
}

/// Read only (the frontmatter): the tip is still an agent tip claimed by
/// `who`'s host and run. Checked before a round's files are copied.
pub fn claim_holds(path: &Path, id: &str, who: &Claimant<'_>) -> Result<(), Dropped> {
    let dropped = |error| Dropped { error };
    let text = frontmatter_text(path).map_err(dropped)?;
    let tip = parse(id, &text)
        .map_err(|e| dropped(WriteError::Refused(OpError::Unreadable(e.describe()))))?;
    ops::check_ours(&tip, who).map_err(|e| dropped(WriteError::Refused(e)))
}

/// Whether the tip at `path` is an agent tip (bounded frontmatter read).
pub fn is_agent_file(path: &Path) -> Result<(), Dropped> {
    agent_gate(path).map_err(|error| Dropped { error })
}

pub fn claim_file(
    path: &Path,
    id: &str,
    who: &Claimant<'_>,
    known: &Known<'_>,
) -> Result<Claimed, Dropped> {
    update_gated(path, &agent_gate, |text| {
        ops::claim(id, text, who, known).map(|c| (c.text.clone(), c))
    })
    .map_err(|error| Dropped { error })
}

pub fn finish_file(
    path: &Path,
    id: &str,
    who: &Claimant<'_>,
    expected_body: &str,
    outcome: &Outcome,
) -> Result<Finished, Dropped> {
    update_gated(path, &agent_gate, |text| {
        ops::finish(id, text, who, expected_body, outcome).map(|f| (f.text.clone(), f))
    })
    .map_err(|error| Dropped { error })
}

//! Claim and finish on the tip file: [`super::ops`] applied through
//! [`super::folder::update`], so a concurrent edit is re-read and the patch
//! re-applied, and a vanished file is reported and never recreated.

use std::path::Path;

use super::block::Outcome;
use super::folder::{update, WriteError};
use super::ops::{self, Claimant, Claimed, Finished, Known, OpError};

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
}

pub fn claim_file(
    path: &Path,
    id: &str,
    who: &Claimant<'_>,
    known: &Known<'_>,
) -> Result<Claimed, Dropped> {
    update(path, |text| {
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
    update(path, |text| {
        ops::finish(id, text, who, expected_body, outcome).map(|f| (f.text.clone(), f))
    })
    .map_err(|error| Dropped { error })
}

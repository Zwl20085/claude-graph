//! `<data dir>/inbox/state.json`: the last scan time and the run ids this
//! machine claimed with (the last [`CLAIMED_MAX`]). A `running` claim of this
//! host is re-run as stale only for a recorded run id (protocol section 9).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::graphgen::inbox_root;

pub const STATE_FILE: &str = "state.json";
/// Most recorded run ids.
pub const CLAIMED_MAX: usize = 500;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Persisted {
    pub last_scan: Option<DateTime<Utc>>,
    /// Oldest first.
    pub claimed: Vec<String>,
}

pub fn state_file(data_dir: &Path) -> PathBuf {
    inbox_root(data_dir).join(STATE_FILE)
}

/// The saved state; empty when missing, unreadable ones are logged.
pub fn load(data_dir: &Path) -> Persisted {
    let file = state_file(data_dir);
    if !file.is_file() {
        return Persisted::default();
    }
    match crate::store::read_json::<Persisted>(&file) {
        Ok(p) => Persisted {
            claimed: bounded(p.claimed),
            ..p
        },
        Err(e) => {
            log::warn!("inbox: ignoring unreadable {STATE_FILE}: {e}");
            Persisted::default()
        }
    }
}

pub fn save(data_dir: &Path, p: &Persisted) {
    if let Err(e) = crate::store::write_json(&state_file(data_dir), p) {
        log::warn!("inbox: could not save {STATE_FILE}: {e}");
    }
}

fn bounded(ids: Vec<String>) -> Vec<String> {
    let skip = ids.len().saturating_sub(CLAIMED_MAX);
    ids.into_iter().skip(skip).collect()
}

/// Add `run` (newest last), keeping at most [`CLAIMED_MAX`].
pub fn remember(claimed: &mut VecDeque<String>, run: &str) {
    claimed.retain(|r| r != run);
    claimed.push_back(run.to_string());
    while claimed.len() > CLAIMED_MAX {
        claimed.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claimed_ids_round_trip_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(tmp.path()), Persisted::default());
        let mut q = VecDeque::new();
        for i in 0..(CLAIMED_MAX + 20) {
            remember(&mut q, &format!("r{i}"));
        }
        remember(&mut q, "r30");
        assert_eq!(q.len(), CLAIMED_MAX);
        assert_eq!(q.back().map(String::as_str), Some("r30"));
        assert_eq!(q.front().map(String::as_str), Some("r20"));
        let p = Persisted {
            last_scan: Some(Utc::now()),
            claimed: q.iter().cloned().collect(),
        };
        save(tmp.path(), &p);
        assert_eq!(load(tmp.path()), p);
        // Older files (last_scan only) load; hand-made long lists are cut.
        std::fs::write(state_file(tmp.path()), r#"{"last_scan": null}"#).unwrap();
        assert!(load(tmp.path()).claimed.is_empty());
        let long: Vec<String> = (0..600).map(|i| format!("x{i}")).collect();
        save(
            tmp.path(),
            &Persisted {
                last_scan: None,
                claimed: long,
            },
        );
        let l = load(tmp.path());
        assert_eq!(
            (l.claimed.len(), l.claimed[0].as_str()),
            (CLAIMED_MAX, "x100")
        );
        std::fs::write(state_file(tmp.path()), "{ broken").unwrap();
        assert_eq!(load(tmp.path()), Persisted::default());
    }
}

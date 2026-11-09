//! One read-only pass over the tips folder: every agent tip with its display
//! state and decision, unreadable files with the reason, and a count of the
//! other tips. A tip is first read only up to its closing fence (at most
//! 64 KiB); the whole file is read only for `kind: agent` tips.

use std::path::{Path, PathBuf};

use crate::contract::{AppError, AppResult, InboxSkipped, InboxTip, InboxTipState};

use super::eligible::{decide, display, Decision, Display, Runner};
use super::folder::{list, read, read_frontmatter, Prefix, FRONTMATTER_CAP};
use super::ops::patchable;
use super::split::split;
use super::tip::{parse, parse_mapping, TipError, KIND_AGENT};

/// Skip reason of an agent tip whose `agent` map cannot be changed safely.
pub const UNPATCHABLE: &str = "cannot patch the frontmatter; edit it by hand";

/// An agent tip as seen by the scan.
#[derive(Debug, Clone)]
pub struct Seen {
    pub view: InboxTip,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct ScanView {
    pub tips: Vec<Seen>,
    pub skipped: Vec<InboxSkipped>,
    pub other_tips: u32,
}

fn state_of(d: Display) -> InboxTipState {
    match d {
        Display::None => InboxTipState::None,
        Display::Running => InboxTipState::Running,
        Display::Queued => InboxTipState::Queued,
        Display::Done => InboxTipState::Done,
        Display::Question => InboxTipState::Question,
        Display::Failed => InboxTipState::Failed,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A tip id the results folder cannot use: Windows drops a trailing `.` or
/// space from folder names, so `agent/<id>/` would name another folder.
pub fn bad_id(id: &str) -> Option<String> {
    id.ends_with(['.', ' '])
        .then(|| "the file name ends with a dot or a space; rename the tip".to_string())
}

/// Read the folder (no writes).
pub fn scan_folder(folder: &Path, me: &Runner<'_>) -> AppResult<ScanView> {
    if !folder.is_dir() {
        return Err(AppError::new(
            "not_found",
            format!("tips folder {} is not an existing folder", folder.display()),
        ));
    }
    let mut view = ScanView::default();
    for e in list(folder)? {
        match see(&e.id, &e.path, me) {
            Ok(Some(seen)) => view.tips.push(seen),
            Ok(None) => view.other_tips += 1,
            Err(skipped) => view.skipped.push(skipped),
        }
    }
    Ok(view)
}

/// `kind` from the bounded frontmatter read.
fn kind_of(path: &Path) -> Result<Option<String>, String> {
    let prefix = read_frontmatter(path).map_err(|e| format!("cannot read: {e}"))?;
    let text = match prefix {
        Prefix::Frontmatter(t) => t,
        Prefix::NoFrontmatter => return Err(TipError::NoFrontmatter.describe()),
        Prefix::NotUtf8 => return Err(TipError::NotUtf8.describe()),
        Prefix::TooLong => {
            return Err(format!(
                "no closing --- within the first {} KiB",
                FRONTMATTER_CAP / 1024
            ))
        }
    };
    let parts = split(&text).ok_or_else(|| TipError::NoFrontmatter.describe())?;
    let map = parse_mapping(parts.frontmatter).map_err(|e| e.describe())?;
    Ok(match map.get("kind") {
        Some(serde_yaml::Value::String(s)) => Some(s.clone()),
        _ => None,
    })
}

/// One file: `Ok(None)` for a tip that is not an agent tip.
pub fn see(id: &str, path: &Path, me: &Runner<'_>) -> Result<Option<Seen>, InboxSkipped> {
    let skip = |reason: String| InboxSkipped {
        file: file_name(path),
        reason,
    };
    if kind_of(path).map_err(skip)?.as_deref() != Some(KIND_AGENT) {
        return Ok(None);
    }
    if let Some(why) = bad_id(id) {
        return Err(skip(why));
    }
    let (bytes, _) = read(path).map_err(|e| skip(format!("cannot read: {e}")))?;
    let text = String::from_utf8(bytes).map_err(|_| skip(TipError::NotUtf8.describe()))?;
    let tip = parse(id, &text).map_err(|e| skip(e.describe()))?;
    if patchable(&text, &tip).is_err() {
        return Err(skip(UNPATCHABLE.into()));
    }
    let decision = decide(&tip, me);
    let agent = tip.agent.clone().unwrap_or_default();
    Ok(Some(Seen {
        view: InboxTip {
            id: tip.id.clone(),
            title: tip.title.clone(),
            state: state_of(display(&tip)),
            eligible: decision.is_eligible(),
            decision: match decision {
                Decision::Eligible(why) => why.to_string(),
                Decision::Skip(why) => why,
            },
            host: agent.host(),
            at: agent.at(),
            run: None,
        },
        path: path.to_path_buf(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;

    fn scan(d: &Path) -> AppResult<ScanView> {
        let empty = HashSet::new();
        scan_folder(
            d,
            &Runner {
                host: "H",
                live: &empty,
                claimed: &empty,
            },
        )
    }

    #[test]
    fn reports_agent_tips_skips_and_others() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::write(d.join("a.md"), "---\ntitle: A\nkind: agent\n---\nDo a\n").unwrap();
        fs::write(d.join("b.md"), "---\ntitle: B\nkind: task\n---\n\u{1}x").unwrap();
        fs::write(d.join("c.md"), [0xffu8, 0x00]).unwrap();
        fs::write(d.join("d.md"), "---\nkind: agent\nagent: [1]\n---\n").unwrap();
        fs::write(d.join("e.md"), "no frontmatter").unwrap();
        let v = scan(d).unwrap();
        assert_eq!(v.tips.len(), 1);
        assert_eq!(v.tips[0].view.state, InboxTipState::Queued);
        assert!(v.tips[0].view.eligible);
        assert_eq!(v.other_tips, 1);
        let files: Vec<&str> = v.skipped.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(files, ["c.md", "d.md", "e.md"]);
        assert!(scan(&d.join("missing")).is_err());
    }

    #[test]
    fn non_agent_tips_are_judged_by_their_frontmatter_only() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        // A task tip whose body is not UTF-8 and huge is a plain other tip.
        let mut task = b"---\nkind: task\n---\n".to_vec();
        task.extend_from_slice(&[0xff; 200_000]);
        fs::write(d.join("task.md"), task).unwrap();
        // Frontmatter never closed within the cap: skipped with the reason.
        fs::write(
            d.join("open.md"),
            format!("---\n{}", "a: b\n".repeat(20_000)),
        )
        .unwrap();
        // Trailing blanks on a fence: no frontmatter (protocol section 9).
        fs::write(d.join("blank.md"), "--- \nkind: agent\n---\nx\n").unwrap();
        // An agent tip that is not UTF-8 past the frontmatter is skipped, not read lossily.
        let mut agent = b"---\nkind: agent\n---\n".to_vec();
        agent.extend_from_slice(&[0xff, 0xfe]);
        fs::write(d.join("bin.md"), agent).unwrap();
        let v = scan(d).unwrap();
        assert_eq!(v.other_tips, 1);
        let reasons: Vec<(String, String)> = v
            .skipped
            .iter()
            .map(|s| (s.file.clone(), s.reason.clone()))
            .collect();
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(reasons
            .iter()
            .any(|(f, r)| f == "open.md" && r.contains("64 KiB")));
        assert!(reasons
            .iter()
            .any(|(f, r)| f == "blank.md" && r.contains("frontmatter")));
        assert!(reasons
            .iter()
            .any(|(f, r)| f == "bin.md" && r.contains("UTF-8")));
    }

    #[test]
    fn unpatchable_and_badly_named_agent_tips_are_skipped_with_a_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::write(
            d.join("comment.md"),
            "---\nkind: agent\nagent:\n  v: 1\n# note\n  state: done\n---\nx\n",
        )
        .unwrap();
        fs::write(d.join("dot..md"), "---\nkind: agent\n---\nx\n").unwrap();
        fs::write(d.join("space .md"), "---\nkind: agent\n---\nx\n").unwrap();
        fs::write(d.join("task.md"), "---\nkind: task\n\"agent\": 1\n---\nx\n").unwrap();
        let v = scan(d).unwrap();
        assert!(v.tips.is_empty());
        assert_eq!(v.other_tips, 1);
        let r = |f: &str| {
            v.skipped
                .iter()
                .find(|s| s.file == f)
                .map(|s| s.reason.clone())
        };
        assert_eq!(r("comment.md").as_deref(), Some(UNPATCHABLE));
        assert!(r("dot..md").unwrap().contains("dot or a space"));
        assert!(r("space .md").unwrap().contains("dot or a space"));
    }
}

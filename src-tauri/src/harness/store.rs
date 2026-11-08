//! `<data_dir>/harnesses.json` = `{ "profiles": [...] }`: custom profiles
//! and overrides of the built-ins (an entry with a built-in id replaces that
//! built-in's editable fields: command, args, env, model). Machine-local,
//! never part of a graph.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::contract::{AppError, AppResult, HarnessProfile};
use crate::harness::profile::{builtins, invalid, validated};
use crate::store::{read_json, write_json};

pub const HARNESSES_FILE: &str = "harnesses.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HarnessFile {
    #[serde(default)]
    pub profiles: Vec<HarnessProfile>,
}

fn read_file(data_dir: &Path) -> HarnessFile {
    let file = data_dir.join(HARNESSES_FILE);
    if !file.is_file() {
        return HarnessFile::default();
    }
    read_json::<HarnessFile>(&file).unwrap_or_else(|e| {
        log::warn!("ignoring unreadable {HARNESSES_FILE}: {e}");
        HarnessFile::default()
    })
}

/// `base` (a built-in) with the editable fields of `over`.
fn overridden(base: HarnessProfile, over: &HarnessProfile) -> HarnessProfile {
    HarnessProfile {
        command: over.command.clone(),
        args: over.args.clone(),
        env: over.env.clone(),
        model: over.model.clone(),
        dsh_profile: over.dsh_profile.clone(),
        ..base
    }
}

/// Built-ins merged with their overrides, then the custom profiles. Entries
/// that fail validation (hand-edited file) are skipped with a warning.
pub fn list(data_dir: &Path) -> Vec<HarnessProfile> {
    let stored: Vec<HarnessProfile> = read_file(data_dir)
        .profiles
        .into_iter()
        .filter_map(|p| match validated(&p) {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!("ignoring harness {:?} in {HARNESSES_FILE}: {e}", p.id);
                None
            }
        })
        .collect();
    let builtin = builtins();
    let is_builtin = |id: &str| builtin.iter().any(|b| b.id == id);
    let merged = builtin.iter().map(|b| {
        stored
            .iter()
            .find(|s| s.id == b.id)
            .map_or_else(|| b.clone(), |s| overridden(b.clone(), s))
    });
    let customs = stored.iter().filter(|s| !is_builtin(&s.id)).cloned();
    merged.chain(customs).collect()
}

/// Validate and store `profile`; returns the new list. A built-in id stores
/// an override (its label and kind stay fixed).
pub fn save(data_dir: &Path, profile: &HarnessProfile) -> AppResult<Vec<HarnessProfile>> {
    let builtin = builtins().into_iter().find(|b| b.id == profile.id);
    let candidate = match &builtin {
        Some(b) => overridden(b.clone(), profile),
        None => profile.clone(),
    };
    let clean = validated(&candidate)?;
    let mut file = read_file(data_dir);
    file.profiles.retain(|p| p.id != clean.id);
    file.profiles.push(clean);
    write_json(&data_dir.join(HARNESSES_FILE), &file)?;
    Ok(list(data_dir))
}

/// Delete a custom profile; returns the new list.
pub fn delete(data_dir: &Path, id: &str) -> AppResult<Vec<HarnessProfile>> {
    if builtins().iter().any(|b| b.id == id) {
        return Err(invalid(format!("{id} is built in and cannot be deleted")));
    }
    let mut file = read_file(data_dir);
    let before = file.profiles.len();
    file.profiles.retain(|p| p.id != id);
    if file.profiles.len() == before {
        return Err(AppError::new("not_found", format!("no harness {id:?}")));
    }
    write_json(&data_dir.join(HARNESSES_FILE), &file)?;
    Ok(list(data_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{HarnessKind, StopMode};
    use crate::harness::profile::CODEX_ID;

    fn custom(id: &str) -> HarnessProfile {
        HarnessProfile {
            id: id.into(),
            label: "Mine".into(),
            kind: HarnessKind::Custom,
            command: Some("node".into()),
            args: vec!["x.mjs".into()],
            env: Default::default(),
            model: None,
            builtin: false,
            idle_marker: None,
            dialog_markers: Vec::new(),
            stop_mode: StopMode::Quiet,
            quiet_ms: None,
            option_pattern: None,
            dialog_hints: Vec::new(),
            dsh_profile: None,
        }
    }

    #[test]
    fn list_save_override_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        assert_eq!(list(data), builtins());
        let after = save(data, &custom("mine")).unwrap();
        assert_eq!(after.len(), 5);
        assert_eq!(after[4].id, "mine");
        // Override a built-in: label and kind stay, args and model change.
        let over = HarnessProfile {
            label: "Renamed".into(),
            kind: HarnessKind::Custom,
            model: Some("gpt-5".into()),
            ..builtins()[2].clone()
        };
        let after = save(data, &over).unwrap();
        let codex = after.iter().find(|p| p.id == CODEX_ID).unwrap();
        assert_eq!(codex.label, "Codex CLI");
        assert_eq!(codex.kind, HarnessKind::Codex);
        assert!(codex.builtin);
        assert_eq!(codex.model.as_deref(), Some("gpt-5"));
        assert_eq!(after.len(), 5, "an override is not a new profile");
        // Delete: customs only.
        assert_eq!(delete(data, CODEX_ID).unwrap_err().code, "invalid_input");
        assert_eq!(delete(data, "nope").unwrap_err().code, "not_found");
        assert_eq!(delete(data, "mine").unwrap().len(), 4);
        let bad = HarnessProfile {
            id: "Bad Id".into(),
            ..custom("x")
        };
        assert_eq!(save(data, &bad).unwrap_err().code, "invalid_input");
    }

    #[test]
    fn hand_edited_bad_entries_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(HARNESSES_FILE),
            r#"{"profiles":[{"id":"BAD","kind":"custom"},{"id":"ok","label":"Ok","kind":"custom","command":"x"}]}"#,
        )
        .unwrap();
        let ids: Vec<String> = list(tmp.path()).into_iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            [
                "claude",
                "claude-deepseek",
                "codex",
                "deepseek-harness",
                "ok"
            ]
        );
        std::fs::write(tmp.path().join(HARNESSES_FILE), "not json").unwrap();
        assert_eq!(list(tmp.path()).len(), 4);
    }
}

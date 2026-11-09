//! Per-user app settings in `<data_dir>/claude-graph/settings.json`.
//!
//! The `claude` executable location lives here, never in graph.json: a graph
//! folder can come from anyone, so a path in it must not decide what the app
//! executes. The value is validated when set and again when loaded (the file
//! could have been edited by hand).

use std::path::{Path, PathBuf};

use crate::contract::{AppError, AppResult, AppSettings};
use crate::paths::{canonical, is_unc_or_device};
use crate::store::{read_json, write_json};

pub const SETTINGS_FILE: &str = "settings.json";

/// `<data_dir>/claude-graph`, where settings.json, trusted.json and recent.json live.
pub fn default_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("claude-graph"))
}

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::new("invalid_input", msg)
}

/// File name looks like the CLI: stem `claude`, or a `.exe`/`.cmd` named `claude*`.
fn looks_like_claude(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    stem == "claude" || (matches!(ext.as_str(), "exe" | "cmd") && stem.starts_with("claude"))
}

/// Validate a `claude_bin` value: an absolute local path (no UNC, no device
/// path) to an existing file named like the CLI. Returns its canonical form.
pub fn validate_claude_bin(value: &str) -> AppResult<PathBuf> {
    let trimmed = value.trim();
    if is_unc_or_device(trimmed) {
        return Err(invalid(format!(
            "{trimmed} is a network or device path; use a local file"
        )));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(invalid(format!("{trimmed} is not an absolute path")));
    }
    if !path.is_file() {
        return Err(AppError::new(
            "not_found",
            format!("{trimmed} is not an existing file"),
        ));
    }
    if !looks_like_claude(&path) {
        return Err(invalid(format!(
            "{trimmed} does not look like the claude CLI (expected claude, claude.exe or claude.cmd)"
        )));
    }
    canonical(&path)
}

/// Validated copy of `settings` (blank `claude_bin` becomes `None`).
pub fn validated(settings: &AppSettings) -> AppResult<AppSettings> {
    let claude_bin = match settings
        .claude_bin
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(v) => Some(validate_claude_bin(v)?.display().to_string()),
        None => None,
    };
    Ok(AppSettings {
        claude_bin,
        inbox: crate::inbox::settings::validated(&settings.inbox)?,
        autostart: settings.autostart,
    })
}

/// Settings from `<data_dir>/settings.json`; defaults if missing or
/// unreadable. The inbox part is re-checked ([`crate::inbox::settings::sanitised`]).
pub fn load(data_dir: &Path) -> AppSettings {
    let file = data_dir.join(SETTINGS_FILE);
    if !file.is_file() {
        return AppSettings::default();
    }
    match read_json::<AppSettings>(&file) {
        Ok(s) => AppSettings {
            inbox: crate::inbox::settings::sanitised(&s.inbox),
            ..s
        },
        Err(e) => {
            log::warn!("ignoring unreadable app settings: {e}");
            AppSettings::default()
        }
    }
}

/// Validate and write atomically; returns what was stored.
pub fn save(data_dir: &Path, settings: &AppSettings) -> AppResult<AppSettings> {
    let clean = validated(settings)?;
    write_json(&data_dir.join(SETTINGS_FILE), &clean)?;
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn set(data: &Path, v: &str) -> AppResult<AppSettings> {
        save(
            data,
            &AppSettings {
                claude_bin: Some(v.to_string()),
                ..AppSettings::default()
            },
        )
    }

    #[test]
    fn old_files_load_and_inbox_settings_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(SETTINGS_FILE), r#"{"claude_bin": null}"#).unwrap();
        let s = load(tmp.path());
        assert_eq!(s, AppSettings::default());
        assert!(!s.inbox.enabled && !s.autostart && s.inbox.interval_hours == 12);
        let tips = tmp.path().join("tips");
        fs::create_dir(&tips).unwrap();
        let wanted = AppSettings {
            inbox: crate::contract::InboxSettings {
                enabled: true,
                folder: tips.display().to_string(),
                interval_hours: 500,
                ..Default::default()
            },
            autostart: true,
            ..AppSettings::default()
        };
        let stored = save(tmp.path(), &wanted).unwrap();
        assert_eq!(stored.inbox.interval_hours, 168);
        assert!(stored.autostart && stored.inbox.enabled);
        assert_eq!(load(tmp.path()), stored);
        // A hand-edited bad folder turns the inbox off on load.
        fs::write(
            tmp.path().join(SETTINGS_FILE),
            r#"{"inbox": {"enabled": true, "folder": "relative", "max_parallel": 9}}"#,
        )
        .unwrap();
        let s = load(tmp.path());
        assert!(!s.inbox.enabled && s.inbox.max_parallel == 4);
    }

    #[test]
    fn round_trip_and_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        assert_eq!(load(&data), AppSettings::default());

        let bin = tmp.path().join("claude.exe");
        fs::write(&bin, b"").unwrap();
        let stored = set(&data, &bin.display().to_string()).unwrap();
        assert!(stored
            .claude_bin
            .as_deref()
            .unwrap()
            .ends_with("claude.exe"));
        assert_eq!(load(&data), stored);

        let shim = tmp.path().join("claude-code.cmd");
        fs::write(&shim, b"").unwrap();
        set(&data, &shim.display().to_string()).unwrap();

        let other = tmp.path().join("calc.exe");
        fs::write(&other, b"").unwrap();
        assert_eq!(
            set(&data, &other.display().to_string()).unwrap_err().code,
            "invalid_input"
        );
        assert_eq!(set(&data, "claude.exe").unwrap_err().code, "invalid_input");
        assert_eq!(
            set(&data, r"\\server\share\claude.exe").unwrap_err().code,
            "invalid_input"
        );
        assert_eq!(
            set(&data, r"\\?\UNC\server\share\claude.exe")
                .unwrap_err()
                .code,
            "invalid_input"
        );
        let missing = tmp.path().join("nope").join("claude.exe");
        assert_eq!(
            set(&data, &missing.display().to_string()).unwrap_err().code,
            "not_found"
        );
        let cleared = set(&data, "  ").unwrap();
        assert_eq!(cleared.claude_bin, None);
        assert_eq!(load(&data), cleared);
    }

    #[test]
    fn unreadable_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(SETTINGS_FILE), "{ broken").unwrap();
        assert_eq!(load(tmp.path()), AppSettings::default());
    }
}

//! Validation of the `inbox` app settings and the folder suggestion.
//!
//! On set: numbers are clamped; a folder must be an absolute local path (no
//! UNC or device form); enabling needs an existing folder, stored in its
//! canonical form. On load: numbers clamped, and a folder that is not a
//! plausible local path turns the inbox off (logged). The suggestion is only
//! shown: nothing is scanned until the owner enables the inbox.

use std::path::{Path, PathBuf};

use crate::contract::{
    AppError, AppResult, InboxSettings, INBOX_INTERVAL_MAX, INBOX_INTERVAL_MIN, INBOX_PARALLEL_MAX,
    INBOX_TIMEOUT_MAX, INBOX_TIMEOUT_MIN,
};
use crate::paths::{canonical, is_unc_or_device};

/// The board's app identifier (its settings live in `%APPDATA%\<id>\settings.json`).
pub const VINDICTIVE_ID: &str = "dev.zhangwentao.vindictive";
/// Folder name under the OneDrive root.
pub const ONEDRIVE_SUBFOLDER: &str = "Vindictive";
/// OneDrive roots, in the order tried.
pub const ONEDRIVE_VARS: [&str; 3] = ["OneDriveConsumer", "OneDrive", "OneDriveCommercial"];

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::new("invalid_input", msg)
}

fn clamped(s: &InboxSettings) -> InboxSettings {
    InboxSettings {
        enabled: s.enabled,
        folder: s.folder.trim().to_string(),
        interval_hours: s
            .interval_hours
            .clamp(INBOX_INTERVAL_MIN, INBOX_INTERVAL_MAX),
        max_parallel: s.max_parallel.clamp(1, INBOX_PARALLEL_MAX),
        timeout_minutes: s
            .timeout_minutes
            .clamp(INBOX_TIMEOUT_MIN, INBOX_TIMEOUT_MAX),
    }
}

fn plausible(folder: &str) -> AppResult<()> {
    if is_unc_or_device(folder) {
        return Err(invalid(format!(
            "{folder} is a network or device path; use a local folder"
        )));
    }
    if !Path::new(folder).is_absolute() {
        return Err(invalid(format!("{folder} is not an absolute path")));
    }
    Ok(())
}

/// Validated copy for saving.
pub fn validated(s: &InboxSettings) -> AppResult<InboxSettings> {
    let c = clamped(s);
    if c.folder.is_empty() {
        if c.enabled {
            return Err(invalid("choose the tips folder before enabling the inbox"));
        }
        return Ok(c);
    }
    plausible(&c.folder)?;
    let path = PathBuf::from(&c.folder);
    if !path.is_dir() {
        if c.enabled {
            return Err(AppError::new(
                "not_found",
                format!("{} is not an existing folder", c.folder),
            ));
        }
        return Ok(c);
    }
    Ok(InboxSettings {
        folder: canonical(&path)?.display().to_string(),
        ..c
    })
}

/// Loaded copy: clamped; an implausible folder turns the inbox off.
pub fn sanitised(s: &InboxSettings) -> InboxSettings {
    let c = clamped(s);
    if c.folder.is_empty() {
        return InboxSettings {
            enabled: false,
            ..c
        };
    }
    match plausible(&c.folder) {
        Ok(()) => c,
        Err(e) => {
            log::warn!("inbox turned off: {}", e.message);
            InboxSettings {
                enabled: false,
                folder: String::new(),
                ..c
            }
        }
    }
}

/// Where the suggestion looks (injectable for tests).
#[derive(Debug, Clone, Default)]
pub struct SuggestEnv {
    /// `%APPDATA%`.
    pub appdata: Option<PathBuf>,
    /// Values of [`ONEDRIVE_VARS`], in order.
    pub onedrive: Vec<Option<PathBuf>>,
}

impl SuggestEnv {
    pub fn from_process() -> Self {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            appdata: var("APPDATA"),
            onedrive: ONEDRIVE_VARS.iter().map(|k| var(k)).collect(),
        }
    }
}

/// The board's own `folder` setting if readable, else `<OneDrive>\Vindictive`
/// under the first OneDrive root (in [`ONEDRIVE_VARS`] order) where that
/// folder already exists, else under the first root set.
pub fn suggest(env: &SuggestEnv) -> Option<String> {
    let from_board = env.appdata.as_ref().and_then(|a| {
        let file = a.join(VINDICTIVE_ID).join("settings.json");
        let text = std::fs::read_to_string(file).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let folder = v.get("folder")?.as_str()?.trim().to_string();
        plausible(&folder).ok().map(|_| folder)
    });
    from_board.or_else(|| {
        let candidates: Vec<PathBuf> = env
            .onedrive
            .iter()
            .flatten()
            .map(|root| root.join(ONEDRIVE_SUBFOLDER))
            .collect();
        candidates
            .iter()
            .find(|c| c.is_dir())
            .or(candidates.first())
            .map(|c| c.display().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(folder: &str, enabled: bool) -> InboxSettings {
        InboxSettings {
            enabled,
            folder: folder.into(),
            ..InboxSettings::default()
        }
    }

    #[test]
    fn set_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().display().to_string();
        assert_eq!(
            validated(&with("", false)).unwrap(),
            InboxSettings::default()
        );
        assert_eq!(
            validated(&with("", true)).unwrap_err().code,
            "invalid_input"
        );
        assert_eq!(
            validated(&with("rel/dir", false)).unwrap_err().code,
            "invalid_input"
        );
        assert_eq!(
            validated(&with(r"\\server\share", false)).unwrap_err().code,
            "invalid_input"
        );
        let missing = tmp.path().join("nope").display().to_string();
        assert_eq!(
            validated(&with(&missing, true)).unwrap_err().code,
            "not_found"
        );
        assert!(validated(&with(&missing, false)).is_ok());
        let ok = validated(&with(&dir, true)).unwrap();
        assert!(ok.enabled && Path::new(&ok.folder).is_dir());
        let wild = InboxSettings {
            interval_hours: 0,
            max_parallel: 99,
            timeout_minutes: 1,
            ..with("", false)
        };
        let c = validated(&wild).unwrap();
        assert_eq!(
            (c.interval_hours, c.max_parallel, c.timeout_minutes),
            (1, 4, 5)
        );
        let c = validated(&InboxSettings {
            interval_hours: 1000,
            max_parallel: 0,
            ..with("", false)
        })
        .unwrap();
        assert_eq!((c.interval_hours, c.max_parallel), (168, 1));
    }

    #[test]
    fn load_rules() {
        assert!(!sanitised(&with("", true)).enabled);
        let s = sanitised(&with("relative", true));
        assert!(!s.enabled && s.folder.is_empty());
        // A plausible folder that is missing right now stays as set.
        let s = sanitised(&with(r"C:\gone\tips", true));
        assert_eq!(s.enabled, cfg!(windows));
    }

    #[test]
    fn suggestion_order() {
        let tmp = tempfile::tempdir().unwrap();
        let appdata = tmp.path().join("appdata");
        let od1 = tmp.path().join("od1");
        let od2 = tmp.path().join("od2");
        let env = SuggestEnv {
            appdata: Some(appdata.clone()),
            onedrive: vec![None, Some(od1.clone()), Some(od2)],
        };
        assert_eq!(
            suggest(&env),
            Some(od1.join("Vindictive").display().to_string())
        );
        let board = appdata.join(VINDICTIVE_ID);
        std::fs::create_dir_all(&board).unwrap();
        let folder = tmp.path().join("tips").display().to_string();
        std::fs::write(
            board.join("settings.json"),
            serde_json::json!({ "folder": folder, "theme": "dark" }).to_string(),
        )
        .unwrap();
        assert_eq!(suggest(&env), Some(folder));
        std::fs::write(board.join("settings.json"), "{ broken").unwrap();
        assert_eq!(
            suggest(&env),
            Some(od1.join("Vindictive").display().to_string())
        );
        assert_eq!(suggest(&SuggestEnv::default()), None);
    }

    /// A machine with personal and work OneDrive: the root that already
    /// holds the board's folder wins over the first root found.
    #[test]
    fn an_existing_board_folder_under_any_onedrive_root_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let personal = tmp.path().join("OneDrive");
        let work = tmp.path().join("OneDrive - Uni");
        std::fs::create_dir_all(work.join(ONEDRIVE_SUBFOLDER)).unwrap();
        std::fs::create_dir_all(&personal).unwrap();
        let env = SuggestEnv {
            appdata: None,
            onedrive: vec![Some(personal.clone()), None, Some(work.clone())],
        };
        assert_eq!(
            suggest(&env),
            Some(work.join(ONEDRIVE_SUBFOLDER).display().to_string())
        );
        std::fs::create_dir_all(personal.join(ONEDRIVE_SUBFOLDER)).unwrap();
        assert_eq!(
            suggest(&env),
            Some(personal.join(ONEDRIVE_SUBFOLDER).display().to_string())
        );
    }
}

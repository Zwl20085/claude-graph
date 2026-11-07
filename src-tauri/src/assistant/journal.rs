//! The `assistant/` folder of a graph: `session.json` (rewritten atomically),
//! `journal.jsonl` (append only, one [`AssistantMessage`] per line) and the
//! per-turn folders `turns/NNNN/`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::contract::{
    AppResult, AssistantKind, AssistantMessage, AssistantRole, AssistantSession, AssistantState,
};
use crate::store::{append, read_json, read_optional, write_json};

pub const ASSISTANT_DIR: &str = "assistant";
pub const SESSION_FILE: &str = "session.json";
pub const JOURNAL_FILE: &str = "journal.jsonl";
pub const CHARTER_FILE: &str = "CHARTER.md";
pub const TURNS_DIR: &str = "turns";
pub const REPLY_FILE: &str = "reply.md";

pub fn assistant_dir(graph_dir: &Path) -> PathBuf {
    graph_dir.join(ASSISTANT_DIR)
}

pub fn session_file(graph_dir: &Path) -> PathBuf {
    assistant_dir(graph_dir).join(SESSION_FILE)
}

pub fn journal_file(graph_dir: &Path) -> PathBuf {
    assistant_dir(graph_dir).join(JOURNAL_FILE)
}

pub fn charter_file(graph_dir: &Path) -> PathBuf {
    assistant_dir(graph_dir).join(CHARTER_FILE)
}

/// `assistant/turns/0001/` for turn 1.
pub fn turn_dir(graph_dir: &Path, turn: u32) -> PathBuf {
    assistant_dir(graph_dir)
        .join(TURNS_DIR)
        .join(format!("{turn:04}"))
}

/// `None` when the graph has no `assistant/session.json` yet.
pub fn read_session(graph_dir: &Path) -> AppResult<Option<AssistantSession>> {
    let path = session_file(graph_dir);
    if path.is_file() {
        read_json(&path).map(Some)
    } else {
        Ok(None)
    }
}

/// The stored session, or an empty idle one.
pub fn session_or_empty(graph_dir: &Path, now: DateTime<Utc>) -> AppResult<AssistantSession> {
    Ok(read_session(graph_dir)?.unwrap_or_else(|| AssistantSession::empty(now)))
}

pub fn write_session(graph_dir: &Path, session: &AssistantSession) -> AppResult<()> {
    write_json(&session_file(graph_dir), session)
}

pub fn new_message(
    turn: u32,
    role: AssistantRole,
    kind: AssistantKind,
    title: Option<String>,
    text: &str,
    now: DateTime<Utc>,
) -> AssistantMessage {
    AssistantMessage {
        id: uuid::Uuid::new_v4().to_string(),
        ts: now,
        turn,
        role,
        kind,
        title,
        text: text.trim().to_string(),
    }
}

pub fn append_messages(graph_dir: &Path, messages: &[AssistantMessage]) -> AppResult<()> {
    if messages.is_empty() {
        return Ok(());
    }
    let lines = messages
        .iter()
        .map(|m| serde_json::to_string(m).map(|l| format!("{l}\n")))
        .collect::<Result<String, _>>()?;
    append(&journal_file(graph_dir), &lines)
}

/// The journal in order; unreadable lines are skipped with a warning.
pub fn read_messages(graph_dir: &Path) -> AppResult<Vec<AssistantMessage>> {
    let text = read_optional(&journal_file(graph_dir))?.unwrap_or_default();
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str(l) {
            Ok(m) => Some(m),
            Err(e) => {
                log::warn!("skipping a bad {JOURNAL_FILE} line: {e}");
                None
            }
        })
        .collect())
}

/// Session plus journal; an empty idle state when `assistant/` is missing.
pub fn read_state(graph_dir: &Path) -> AppResult<AssistantState> {
    Ok(AssistantState {
        session: session_or_empty(graph_dir, Utc::now())?,
        messages: read_messages(graph_dir)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::AssistantStatus;

    #[test]
    fn missing_folder_is_an_empty_idle_state() {
        let tmp = tempfile::tempdir().unwrap();
        let state = read_state(tmp.path()).unwrap();
        assert_eq!(state.session.status, AssistantStatus::Idle);
        assert!(state.messages.is_empty());
        assert!(
            !assistant_dir(tmp.path()).exists(),
            "reading creates nothing"
        );
    }

    #[test]
    fn session_and_journal_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let session = AssistantSession {
            status: AssistantStatus::Waiting,
            turns: 3,
            ..AssistantSession::empty(now)
        };
        write_session(tmp.path(), &session).unwrap();
        let a = new_message(
            1,
            AssistantRole::User,
            AssistantKind::Instruction,
            None,
            "  go  ",
            now,
        );
        let b = new_message(
            1,
            AssistantRole::Scheduler,
            AssistantKind::Stage,
            Some("x".into()),
            "ok",
            now,
        );
        append_messages(tmp.path(), std::slice::from_ref(&a)).unwrap();
        append_messages(tmp.path(), std::slice::from_ref(&b)).unwrap();
        append_messages(tmp.path(), &[]).unwrap();
        std::fs::write(
            journal_file(tmp.path()),
            format!(
                "{}not json\n",
                std::fs::read_to_string(journal_file(tmp.path())).unwrap()
            ),
        )
        .unwrap();
        let state = read_state(tmp.path()).unwrap();
        assert_eq!(state.session, session);
        assert_eq!(state.messages, vec![a.clone(), b]);
        assert_eq!(a.text, "go");
        assert_ne!(state.messages[0].id, state.messages[1].id);
    }

    #[test]
    fn turn_folders_are_zero_padded() {
        let d = turn_dir(Path::new("/g"), 7);
        assert!(d.ends_with(Path::new("assistant/turns/0007")));
    }
}

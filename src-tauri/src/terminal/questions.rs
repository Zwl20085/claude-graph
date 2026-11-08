//! `runs/<run>/<node>/questions.jsonl`: one [`QuestionRecord`] per line, in
//! the order the prompts appeared. An answer is written into its record (the
//! file is small and rewritten whole, atomically).

use std::path::Path;

use crate::contract::{AppResult, QuestionAnswer, QuestionRecord};
use crate::store::{read_optional, write_atomic};

pub const QUESTIONS_FILE: &str = "questions.jsonl";

/// The records of a node's run folder (unreadable lines skipped, logged).
pub fn read(node_run_dir: &Path) -> Vec<QuestionRecord> {
    let path = node_run_dir.join(QUESTIONS_FILE);
    let text = match read_optional(&path) {
        Ok(t) => t.unwrap_or_default(),
        Err(e) => {
            log::warn!("reading {}: {e}", path.display());
            return Vec::new();
        }
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str(l) {
            Ok(r) => Some(r),
            Err(e) => {
                log::warn!("{}: skipping a bad line: {e}", path.display());
                None
            }
        })
        .collect()
}

fn write(node_run_dir: &Path, records: &[QuestionRecord]) -> AppResult<()> {
    let mut text = String::new();
    for r in records {
        text.push_str(&serde_json::to_string(r)?);
        text.push('\n');
    }
    write_atomic(&node_run_dir.join(QUESTIONS_FILE), text.as_bytes())
}

/// Append `record` (a record with the same id is replaced).
pub fn add(node_run_dir: &Path, record: QuestionRecord) -> AppResult<()> {
    let mut records: Vec<QuestionRecord> = read(node_run_dir)
        .into_iter()
        .filter(|r| r.id != record.id)
        .collect();
    records.push(record);
    write(node_run_dir, &records)
}

/// Set the answer of record `id` (no such record: nothing changes).
pub fn answer(node_run_dir: &Path, id: &str, answer: QuestionAnswer) -> AppResult<()> {
    let records: Vec<QuestionRecord> = read(node_run_dir)
        .into_iter()
        .map(|r| {
            if r.id == id {
                QuestionRecord {
                    answer: Some(answer.clone()),
                    ..r
                }
            } else {
                r
            }
        })
        .collect();
    write(node_run_dir, &records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{AnswerVia, PromptSource, QuestionKind};
    use chrono::Utc;

    fn rec(id: &str) -> QuestionRecord {
        QuestionRecord {
            id: id.into(),
            ts: Utc::now(),
            kind: QuestionKind::Dialog,
            source: PromptSource::Screen,
            question: "Pick?".into(),
            header: None,
            options: Vec::new(),
            selected: Some(0),
            offer: false,
            answer: None,
        }
    }

    #[test]
    fn records_append_and_take_their_answer() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read(tmp.path()).is_empty());
        add(tmp.path(), rec("a")).unwrap();
        add(tmp.path(), rec("b")).unwrap();
        let a = QuestionAnswer {
            ts: Utc::now(),
            via: AnswerVia::App,
            text: None,
            index: Some(1),
        };
        answer(tmp.path(), "a", a.clone()).unwrap();
        answer(tmp.path(), "zz", a.clone()).unwrap();
        let all = read(tmp.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].answer, Some(a));
        assert_eq!(all[1].answer, None);
        let text = std::fs::read_to_string(tmp.path().join(QUESTIONS_FILE)).unwrap();
        assert_eq!(text.lines().count(), 2);
        std::fs::write(tmp.path().join(QUESTIONS_FILE), format!("{text}not json\n")).unwrap();
        assert_eq!(read(tmp.path()).len(), 2);
    }
}

//! One scheduler turn: prompt.md, the command line, and recording the outcome.
//!
//! Command line (nothing else, ever):
//! `-p <INSTRUCTION> --output-format stream-json --verbose --add-dir <graph dir>
//! [--resume <session id>]`, cwd = graph dir, prompt on stdin.

use std::path::Path;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::claude_bin::ClaudeCommand;
use crate::contract::{
    AppResult, AssistantKind, AssistantMessage, AssistantRole, AssistantSession, AssistantState,
    AssistantStatus, NodeStatus,
};
use crate::run_store::PROMPT_FILE;
use crate::runner::{TurnOutcome, TurnSpec};
use crate::store::{read_graph, write_atomic};
use crate::turns::checked_session;

use super::charter::load_or_install;
use super::journal::{
    append_messages, new_message, read_state, session_or_empty, turn_dir, write_session, REPLY_FILE,
};
use super::parse::{parse_reply, status_after};
use super::render::render_workflow;
use super::{NODE_ID, RUN_ID};

/// Positional instruction of every scheduler turn.
pub const INSTRUCTION: &str = "You are the work scheduler for this folder. Read the message on stdin and act on it. Reply only in the sections the charter defines.";
/// Appended to the app error listing denied tools.
pub const DENIAL_HINT: &str =
    "Set permissions.defaultMode (or allow rules) in your Claude Code settings";
/// Appended when a resumed turn fails over its session.
pub const RESET_HINT: &str = "Reset the session to start again";
const EMPTY_REPLY: &str = "the scheduler returned an empty reply";

/// The scheduler's arguments: no model, permission mode, tools or turn limit.
/// `resume` must already be checked with [`checked_session`].
pub fn build_args(graph_dir: &Path, resume: Option<&str>) -> Vec<String> {
    let base = [
        "-p",
        INSTRUCTION,
        "--output-format",
        "stream-json",
        "--verbose",
        "--add-dir",
    ]
    .map(str::to_string);
    let resume = resume
        .into_iter()
        .flat_map(|id| ["--resume".to_string(), id.to_string()]);
    base.into_iter()
        .chain([graph_dir.display().to_string()])
        .chain(resume)
        .collect()
}

pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// prompt.md of a fresh session: charter, workflow, instruction.
pub fn fresh_prompt(charter: &str, workflow: &str, text: &str) -> String {
    format!(
        "{}\n\n## Workflow\n\n{workflow}\n\n## Instruction\n\n{text}\n",
        charter.trim_end()
    )
}

/// prompt.md of a resumed session: the workflow only when it changed.
pub fn resumed_prompt(workflow: Option<&str>, text: &str) -> String {
    let updated = workflow
        .map(|w| format!("## Workflow (updated)\n\n{w}\n\n"))
        .unwrap_or_default();
    format!("{updated}## Message from the assistant\n\n{text}\n")
}

/// A turn ready to spawn: its spec, the session to store (status `working`)
/// and the user's message to append.
pub struct PreparedTurn {
    pub spec: TurnSpec,
    pub session: AssistantSession,
    pub message: AssistantMessage,
    pub turn: u32,
    pub resumed: bool,
}

/// Read graph.json, render the workflow, write `turns/NNNN/prompt.md` (and the
/// charter on a fresh session). Nothing is written to session or journal here.
pub fn prepare(
    graph_dir: &Path,
    previous: &AssistantSession,
    text: &str,
    claude: ClaudeCommand,
    now: DateTime<Utc>,
) -> AppResult<PreparedTurn> {
    let graph = read_graph(graph_dir)?;
    let workflow = render_workflow(&graph);
    let hash = sha256_hex(&workflow);
    let resume = previous
        .session_id
        .as_deref()
        .map(checked_session)
        .transpose()?;
    let prompt = match resume {
        None => fresh_prompt(&load_or_install(graph_dir)?, &workflow, text),
        Some(_) => {
            let changed = previous.workflow_hash.as_deref() != Some(hash.as_str());
            resumed_prompt(changed.then_some(workflow.as_str()), text)
        }
    };
    let turn = previous.turns.saturating_add(1);
    let out_dir = turn_dir(graph_dir, turn);
    write_atomic(&out_dir.join(PROMPT_FILE), prompt.as_bytes())?;
    let kind = if previous.status == AssistantStatus::Waiting {
        AssistantKind::Answer
    } else {
        AssistantKind::Instruction
    };
    Ok(PreparedTurn {
        spec: TurnSpec {
            run_id: RUN_ID.to_string(),
            node_id: NODE_ID.to_string(),
            claude,
            args: build_args(graph_dir, resume),
            stdin_text: prompt,
            cwd: graph_dir.to_path_buf(),
            out_dir,
            required: Vec::new(),
            fresh_since: None,
        },
        session: AssistantSession {
            status: AssistantStatus::Working,
            turns: turn,
            workflow_hash: Some(hash),
            last_error: None,
            updated_at: now,
            ..previous.clone()
        },
        message: new_message(turn, AssistantRole::User, kind, None, text, now),
        turn,
        resumed: resume.is_some(),
    })
}

fn app_message(turn: u32, kind: AssistantKind, text: &str, now: DateTime<Utc>) -> AssistantMessage {
    new_message(turn, AssistantRole::App, kind, None, text, now)
}

fn denial_message(turn: u32, tools: &[String], now: DateTime<Utc>) -> Option<AssistantMessage> {
    let unique = tools.iter().fold(Vec::<&str>::new(), |acc, t| {
        if acc.contains(&t.as_str()) {
            acc
        } else {
            acc.into_iter().chain([t.as_str()]).collect()
        }
    });
    (!unique.is_empty()).then(|| {
        let text = format!(
            "The scheduler was denied these tools: {}. {DENIAL_HINT}.",
            unique.join(", ")
        );
        app_message(turn, AssistantKind::Error, &text, now)
    })
}

fn mentions_session(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    r.contains("no conversation found") || r.contains("session")
}

/// Session and new journal messages after a finished turn (pure).
pub fn evaluate(
    before: &AssistantSession,
    turn: u32,
    resumed: bool,
    outcome: &TurnOutcome,
    now: DateTime<Utc>,
) -> (AssistantSession, Vec<AssistantMessage>) {
    let base = AssistantSession {
        model: outcome.model.clone().or_else(|| before.model.clone()),
        updated_at: now,
        ..before.clone()
    };
    let kept_session = outcome
        .session_id
        .clone()
        .or_else(|| before.session_id.clone());
    let denied = denial_message(turn, &outcome.permission_denials, now);
    let failed = |reason: String, session_id: Option<String>| {
        let msg = app_message(turn, AssistantKind::Error, &reason, now);
        let session = AssistantSession {
            session_id,
            status: AssistantStatus::Failed,
            last_error: Some(reason),
            ..base.clone()
        };
        (session, [msg].into_iter().chain(denied.clone()).collect())
    };
    if outcome.status == NodeStatus::Cancelled {
        let msg = app_message(
            turn,
            AssistantKind::System,
            &format!("Turn {turn} cancelled"),
            now,
        );
        let session = AssistantSession {
            session_id: kept_session,
            status: AssistantStatus::Cancelled,
            last_error: None,
            ..base
        };
        return (session, vec![msg]);
    }
    if outcome.is_error {
        let reason = outcome
            .reason
            .clone()
            .unwrap_or_else(|| "the scheduler turn failed".to_string());
        let reason = if resumed && mentions_session(&reason) {
            format!("{reason}. {RESET_HINT}")
        } else {
            reason
        };
        // A failed resume keeps the session it tried; a failed fresh turn keeps
        // whatever session the CLI started.
        let session_id = before
            .session_id
            .clone()
            .or_else(|| outcome.session_id.clone());
        return failed(reason, session_id);
    }
    let sections = parse_reply(outcome.final_text.as_deref().unwrap_or(""));
    if sections.is_empty() {
        return failed(EMPTY_REPLY.to_string(), kept_session);
    }
    let session = AssistantSession {
        session_id: kept_session,
        status: status_after(&sections),
        last_error: None,
        ..base
    };
    let messages = sections
        .into_iter()
        .map(|(kind, title, text)| {
            new_message(turn, AssistantRole::Scheduler, kind, title, &text, now)
        })
        .chain(denied)
        .collect();
    (session, messages)
}

/// Write reply.md, append the turn's messages, rewrite session.json; returns the new state.
pub fn record(
    graph_dir: &Path,
    turn: u32,
    resumed: bool,
    outcome: &TurnOutcome,
    now: DateTime<Utc>,
) -> AppResult<AssistantState> {
    if let Some(text) = &outcome.final_text {
        write_atomic(&turn_dir(graph_dir, turn).join(REPLY_FILE), text.as_bytes())?;
    }
    let before = session_or_empty(graph_dir, now)?;
    let (session, messages) = evaluate(&before, turn, resumed, outcome, now);
    append_messages(graph_dir, &messages)?;
    write_session(graph_dir, &session)?;
    read_state(graph_dir)
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;

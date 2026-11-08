//! The app side of "Ask me in the app": registrations (one per run or
//! assistant turn that uses `ask_app`), the pending prompts in arrival order,
//! answers, remembered scopes, and cancel. The loopback listener that feeds
//! [`Approvals::submit`] lives in [`super::listen`].

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::Utc;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::contract::{
    AppError, AppResult, NodeEvent, NodeEventKind, PermissionAnswer, PermissionAnswered,
    PermissionPrompt, PermissionScope,
};
use crate::events::Emitter;

use super::registration::Registration;
use super::wire::{allow, deny, match_key, parse_call, summary, token_eq, MatchKey, RelayRequest};
use super::{scopes, CANCELLED, MAX_PENDING};

/// Longest node id accepted from the relay.
const NODE_ID_MAX: usize = 200;
/// Deny message when the asking process went away before an answer.
const GONE: &str = "the asking process went away";

struct Reg {
    run_id: String,
    graph_dir: PathBuf,
    emitter: Arc<dyn Emitter>,
}

struct Pending {
    prompt: PermissionPrompt,
    reg: String,
    graph_dir: PathBuf,
    key: MatchKey,
    emitter: Arc<dyn Emitter>,
    tx: oneshot::Sender<Value>,
}

#[derive(Default)]
struct State {
    addr: Option<SocketAddr>,
    regs: std::collections::HashMap<String, Reg>,
    /// Oldest first.
    pending: Vec<Pending>,
    /// "Allow for this node": (registration, node id, call).
    node_allow: HashSet<(String, String, MatchKey)>,
}

pub(crate) struct Inner {
    pub(super) token: String,
    data_dir: PathBuf,
    state: Mutex<State>,
}

/// Shared approval state of the app (cheap to clone).
#[derive(Clone)]
pub struct Approvals {
    pub(crate) inner: Arc<Inner>,
}

/// What [`Approvals::submit`] decided.
pub enum Submitted {
    /// Answer at once with this decision.
    Now(Value),
    /// Wait for the user's answer to prompt `id`.
    Wait(String, oneshot::Receiver<Value>),
}

fn new_token() -> String {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    a.as_bytes()
        .iter()
        .chain(b.as_bytes())
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn scope_word(scope: PermissionScope) -> &'static str {
    match scope {
        PermissionScope::Once => "once",
        PermissionScope::Node => "node",
        PermissionScope::Graph => "graph",
    }
}

fn node_event(emitter: &dyn Emitter, prompt: &PermissionPrompt, kind: NodeEventKind, text: String) {
    emitter.node_event(&NodeEvent {
        run_id: prompt.run_id.clone(),
        node_id: prompt.node_id.clone(),
        kind,
        text,
        ts: Utc::now(),
    });
}

fn prompt_text(prompt: &PermissionPrompt) -> String {
    format!("{}: {}", prompt.tool_name, prompt.summary)
}

/// Send `decision` and tell the frontend and the log panel.
fn resolve(p: Pending, decision: Value, text: String) {
    if p.tx.send(decision).is_err() {
        log::debug!("approval {}: the asker left before the answer", p.prompt.id);
    }
    node_event(
        &*p.emitter,
        &p.prompt,
        NodeEventKind::PermissionAnswered,
        text,
    );
    p.emitter
        .permission_answered(&PermissionAnswered { id: p.prompt.id });
}

fn denied_text(message: &str) -> String {
    format!("denied (once): {message}")
}

impl Approvals {
    /// `data_dir` is the app's data folder (`approvals.json` lives there).
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                token: new_token(),
                data_dir,
                state: Mutex::new(State::default()),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn token_matches(&self, token: &str) -> bool {
        token_eq(token, &self.inner.token)
    }

    /// The listener address, started on first use.
    fn addr(&self) -> AppResult<SocketAddr> {
        let mut st = self.lock();
        if let Some(addr) = st.addr {
            return Ok(addr);
        }
        let addr = super::listen::start(self)?;
        st.addr = Some(addr);
        Ok(addr)
    }

    /// Register a run (or an assistant turn, `run_id` "assistant") whose turns
    /// use `ask_app`. Dropping the registration denies what it left pending
    /// and forgets its "allow for this node" answers.
    pub fn register(
        &self,
        run_id: &str,
        graph_dir: &Path,
        exe: &Path,
        emitter: Arc<dyn Emitter>,
    ) -> AppResult<Registration> {
        let addr = self.addr()?;
        let id = uuid::Uuid::new_v4().to_string();
        let reg = Reg {
            run_id: run_id.to_string(),
            graph_dir: graph_dir.to_path_buf(),
            emitter,
        };
        self.lock().regs.insert(id.clone(), reg);
        Ok(Registration {
            approvals: self.clone(),
            id,
            run_id: run_id.to_string(),
            addr,
            exe: exe.to_path_buf(),
        })
    }

    /// Pending prompts, oldest first.
    pub fn list(&self) -> Vec<PermissionPrompt> {
        self.lock()
            .pending
            .iter()
            .map(|p| p.prompt.clone())
            .collect()
    }

    /// A relayed request (token already checked): auto-answer from a
    /// remembered scope, deny, or queue it for the user.
    pub(crate) fn submit(&self, req: &RelayRequest) -> Submitted {
        let call = match parse_call(&req.arguments) {
            Ok(c) => c,
            Err(e) => return Submitted::Now(deny(&e)),
        };
        let node_id = req.node_id.trim();
        if node_id.is_empty() || node_id.chars().count() > NODE_ID_MAX {
            return Submitted::Now(deny("permission request has an invalid node id"));
        }
        let (emitter, graph_dir, run_id, node_ok, key) = {
            let st = self.lock();
            let Some(reg) = st.regs.get(&req.reg).filter(|r| r.run_id == req.run_id) else {
                return Submitted::Now(deny("no active Claude Graph run for this request"));
            };
            let key = match_key(&call.tool_name, &call.input);
            let node_ok =
                st.node_allow
                    .contains(&(req.reg.clone(), node_id.to_string(), key.clone()));
            (
                reg.emitter.clone(),
                reg.graph_dir.clone(),
                reg.run_id.clone(),
                node_ok,
                key,
            )
        };
        let prompt = PermissionPrompt {
            id: uuid::Uuid::new_v4().to_string(),
            run_id,
            node_id: node_id.to_string(),
            summary: summary(&call.tool_name, &call.input),
            tool_name: call.tool_name,
            input: call.input,
            suggestions: call.suggestions,
            ts: Utc::now(),
        };
        let auto = if node_ok {
            Some(PermissionScope::Node)
        } else if scopes::allows(&self.inner.data_dir, &graph_dir, &key) {
            Some(PermissionScope::Graph)
        } else {
            None
        };
        if let Some(scope) = auto {
            let text = format!("allowed ({})", scope_word(scope));
            node_event(
                &*emitter,
                &prompt,
                NodeEventKind::PermissionPrompt,
                prompt_text(&prompt),
            );
            node_event(&*emitter, &prompt, NodeEventKind::PermissionAnswered, text);
            emitter.permission_answered(&PermissionAnswered {
                id: prompt.id.clone(),
            });
            return Submitted::Now(allow(&prompt.input));
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut st = self.lock();
            // The registration may have closed since it was checked: its
            // close denies only the prompts already pending.
            if !st.regs.contains_key(&req.reg) {
                return Submitted::Now(deny("no active Claude Graph run for this request"));
            }
            if st.pending.len() >= MAX_PENDING {
                return Submitted::Now(deny(&format!(
                    "too many approvals are waiting in Claude Graph ({MAX_PENDING}); denied"
                )));
            }
            st.pending.push(Pending {
                prompt: prompt.clone(),
                reg: req.reg.clone(),
                graph_dir,
                key,
                emitter: emitter.clone(),
                tx,
            });
        }
        node_event(
            &*emitter,
            &prompt,
            NodeEventKind::PermissionPrompt,
            prompt_text(&prompt),
        );
        emitter.permission_prompt(&prompt);
        Submitted::Wait(prompt.id, rx)
    }

    /// The user's answer. Allowing with `node` or `graph` scope remembers the
    /// call and also answers every other pending prompt it now covers.
    pub fn answer(&self, answer: &PermissionAnswer) -> AppResult<()> {
        let id = answer.id.trim();
        if id.is_empty() {
            return Err(AppError::new("invalid_input", "answer has no prompt id"));
        }
        let scope = answer.scope;
        let (first, covered) = {
            let mut st = self.lock();
            let Some(idx) = st.pending.iter().position(|p| p.prompt.id == id) else {
                return Err(AppError::new(
                    "not_found",
                    format!("permission prompt {id} is not waiting (already answered?)"),
                ));
            };
            let first = st.pending.remove(idx);
            let covers = |p: &Pending| {
                answer.allow
                    && p.key == first.key
                    && match scope {
                        PermissionScope::Once => false,
                        PermissionScope::Node => {
                            p.reg == first.reg && p.prompt.node_id == first.prompt.node_id
                        }
                        PermissionScope::Graph => p.graph_dir == first.graph_dir,
                    }
            };
            let (covered, kept): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut st.pending)
                .into_iter()
                .partition(covers);
            st.pending = kept;
            if answer.allow && scope == PermissionScope::Node {
                let entry = (
                    first.reg.clone(),
                    first.prompt.node_id.clone(),
                    first.key.clone(),
                );
                st.node_allow.insert(entry);
            }
            (first, covered)
        };
        if answer.allow && scope == PermissionScope::Graph {
            if let Err(e) = scopes::remember(&self.inner.data_dir, &first.graph_dir, &first.key) {
                log::warn!(
                    "remembering an approval for {}: {e}",
                    first.graph_dir.display()
                );
            }
        }
        let word = scope_word(scope);
        let message = answer.message.as_deref().unwrap_or_default().trim();
        let (decision, text) = if answer.allow {
            (allow(&first.prompt.input), format!("allowed ({word})"))
        } else if message.is_empty() {
            (deny(message), format!("denied ({word})"))
        } else {
            (deny(message), format!("denied ({word}): {message}"))
        };
        resolve(first, decision, text);
        for p in covered {
            let decision = allow(&p.prompt.input);
            resolve(p, decision, format!("allowed ({word})"));
        }
        Ok(())
    }

    /// Deny (with `message`) every pending prompt for which `pick` holds.
    fn deny_where(&self, pick: impl Fn(&Pending) -> bool, message: &str) -> usize {
        let picked: Vec<Pending> = {
            let mut st = self.lock();
            let (picked, kept) = std::mem::take(&mut st.pending)
                .into_iter()
                .partition(|p| pick(p));
            st.pending = kept;
            picked
        };
        let n = picked.len();
        for p in picked {
            resolve(p, deny(message), denied_text(message));
        }
        n
    }

    /// Run cancelled: deny its pending prompts with "cancelled".
    pub fn cancel_run(&self, run_id: &str) -> usize {
        self.deny_where(|p| p.prompt.run_id == run_id, CANCELLED)
    }

    /// Node cancelled: deny its pending prompts with "cancelled".
    pub fn cancel_node(&self, run_id: &str, node_id: &str) -> usize {
        self.deny_where(
            |p| p.prompt.run_id == run_id && p.prompt.node_id == node_id,
            CANCELLED,
        )
    }

    /// Assistant turn of `graph_dir` cancelled.
    pub fn cancel_assistant(&self, graph_dir: &Path) -> usize {
        self.deny_where(
            |p| p.prompt.run_id == crate::assistant::RUN_ID && p.graph_dir == graph_dir,
            CANCELLED,
        )
    }

    /// App exit: deny everything.
    pub fn cancel_all(&self) -> usize {
        self.deny_where(|_| true, CANCELLED)
    }

    /// The asker of prompt `id` closed its connection.
    pub(crate) fn abandon(&self, id: &str) {
        self.deny_where(|p| p.prompt.id == id, GONE);
    }

    pub(super) fn close(&self, reg: &str) {
        self.deny_where(|p| p.reg == reg, CANCELLED);
        let mut st = self.lock();
        st.node_allow.retain(|(r, _, _)| r != reg);
        st.regs.remove(reg);
    }
}

//! What the inbox runner depends on, injectable for tests: the app state,
//! the emitters, this machine's name, the clock, the settings and the
//! folder suggestion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::NaiveDateTime;

use crate::contract::{
    AppResult, AssistantState, InboxSettings, InboxStatus, NodeEvent, PermissionAnswered,
    PermissionPrompt, PtyData, RunState, SessionPrompt, SessionPromptCleared,
};
use crate::events::{Emitter, WindowSpec};
use crate::run_api::AppState;

/// Receives the whole inbox status after every change (`inbox:update`).
pub trait InboxSink: Send + Sync {
    fn update(&self, status: &InboxStatus);
}

/// A sink that drops everything.
pub struct NullSink;

impl InboxSink for NullSink {
    fn update(&self, _status: &InboxStatus) {}
}

/// The settings a scan and its runs use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    pub enabled: bool,
    pub folder: PathBuf,
    pub max_parallel: usize,
    pub timeout: Duration,
    pub interval: Duration,
}

impl RunConfig {
    pub fn from_settings(s: &InboxSettings) -> Self {
        Self {
            enabled: s.enabled && !s.folder.trim().is_empty(),
            folder: PathBuf::from(s.folder.trim()),
            max_parallel: s.max_parallel.max(1) as usize,
            timeout: Duration::from_secs(u64::from(s.timeout_minutes) * 60),
            interval: Duration::from_secs(u64::from(s.interval_hours) * 3600),
        }
    }
}

pub type Clock = Arc<dyn Fn() -> NaiveDateTime + Send + Sync>;
pub type ConfigFn = Arc<dyn Fn() -> RunConfig + Send + Sync>;
pub type SuggestFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct InboxEnv {
    pub app: AppState,
    /// For the runs (session windows, run events).
    pub emitter: Arc<dyn Emitter>,
    pub sink: Arc<dyn InboxSink>,
    pub host: String,
    pub clock: Clock,
    pub config: ConfigFn,
    pub suggest: SuggestFn,
    /// How often a run's watcher looks at its node.
    pub poll: Duration,
    /// Delay of a re-scan when a graph folder was still busy (60 s in the app).
    pub retry_delay: Duration,
}

impl InboxEnv {
    /// The app's environment: settings from the app data dir, the real host
    /// name, local time, the suggestion from the process env.
    pub fn for_app(app: AppState, emitter: Arc<dyn Emitter>, sink: Arc<dyn InboxSink>) -> Self {
        let st = app.clone();
        Self {
            app,
            emitter,
            sink,
            host: super::host::this_host(),
            clock: Arc::new(|| chrono::Local::now().naive_local()),
            config: Arc::new(move || RunConfig::from_settings(&st.app_settings().inbox)),
            suggest: Arc::new(|| {
                super::settings::suggest(&super::settings::SuggestEnv::from_process())
            }),
            poll: Duration::from_millis(500),
            retry_delay: Duration::from_secs(60),
        }
    }
}

/// Forwards everything to the app's emitter and keeps the run's latest state.
pub struct Tap {
    inner: Arc<dyn Emitter>,
    latest: Mutex<Option<RunState>>,
}

impl Tap {
    pub fn new(inner: Arc<dyn Emitter>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            latest: Mutex::new(None),
        })
    }

    pub fn latest(&self) -> Option<RunState> {
        self.latest
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl Emitter for Tap {
    fn run_state(&self, state: &RunState) {
        *self.latest.lock().unwrap_or_else(|p| p.into_inner()) = Some(state.clone());
        self.inner.run_state(state);
    }
    fn node_event(&self, event: &NodeEvent) {
        self.inner.node_event(event);
    }
    fn assistant_update(&self, state: &AssistantState) {
        self.inner.assistant_update(state);
    }
    fn pty_data(&self, label: &str, data: &PtyData) {
        self.inner.pty_data(label, data);
    }
    fn open_window(&self, spec: &WindowSpec) -> AppResult<()> {
        self.inner.open_window(spec)
    }
    fn close_window(&self, label: &str) -> AppResult<()> {
        self.inner.close_window(label)
    }
    fn permission_prompt(&self, prompt: &PermissionPrompt) {
        self.inner.permission_prompt(prompt);
    }
    fn permission_answered(&self, answered: &PermissionAnswered) {
        self.inner.permission_answered(answered);
    }
    fn session_prompt(&self, prompt: &SessionPrompt) {
        self.inner.session_prompt(prompt);
    }
    fn session_prompt_cleared(&self, cleared: &SessionPromptCleared) {
        self.inner.session_prompt_cleared(cleared);
    }
}

/**
 * IPC contract between the React frontend and the Rust backend.
 *
 * Mirrors `src-tauri/src/contract.rs` one-to-one (docs/ARCHITECTURE.md §2, §6).
 * Field names are snake_case on both sides. Timestamps are RFC 3339 UTC strings
 * (chrono `DateTime<Utc>` on the Rust side). Rust `Option<T>` is `T | null`.
 */

export * from './prompt';
import type { QuestionRecord } from './prompt';

export const GRAPH_VERSION = 1;

/** Event: full RunState whenever any node status changes. */
export const EVENT_RUN_STATE = 'run:state';
/** Event: one summarised stream-json line of a running node. */
export const EVENT_NODE_EVENT = 'node:event';
/** Event (terminal mode): one chunk of a session's pty output, sent to that session's window. */
export const EVENT_PTY_DATA = 'pty:data';

// ---------------------------------------------------------------------------
// Graph (graph.json)
// ---------------------------------------------------------------------------

export const MODELS = ['sonnet', 'opus', 'haiku', 'inherit'] as const;
/** `inherit` on a node means `settings.default_model`. */
export type Model = (typeof MODELS)[number];

export const PERMISSION_MODES = [
  'claude_settings',
  'ask_app',
  'acceptEdits',
  'default',
  'plan',
  'dontAsk',
  'bypassPermissions',
  'inherit',
] as const;
/**
 * `inherit` on a node means `settings.default_permission_mode`.
 * `claude_settings` passes no `--permission-mode` flag: the user's own Claude
 * Code settings decide (the default of new graphs).
 * `ask_app` (headless engine): a tool call that needs approval is relayed to
 * the app, which asks the user in a dialog (ASKAPP_CONTRACT). Not elevated.
 */
export type PermissionMode = (typeof PERMISSION_MODES)[number];

export const RUN_MODES = ['terminal', 'headless'] as const;
/**
 * terminal: every connected group of nodes is one interactive Claude Code
 * session in its own window (default); headless: the `claude -p` engine.
 */
export type RunMode = (typeof RUN_MODES)[number];
export const DEFAULT_RUN_MODE: RunMode = 'terminal';

export interface Settings {
  /** Missing in older graph.json files: treat as `terminal` (see `runModeOf`). */
  run_mode?: RunMode;
  max_parallel: number;
  /** Capacity per resource tag. Unknown tags have capacity 1. */
  resources: Record<string, number>;
  stop_on_failure: boolean;
  default_model: Model;
  default_permission_mode: PermissionMode;
  /**
   * Kept for compatibility with older graph.json files only. The backend
   * ignores it and the UI hides it: the claude path is a per-machine
   * setting (`AppSettings.claude_bin`).
   */
  claude_bin: string | null;
  /**
   * Default harness profile id of the graph's nodes. Missing in older
   * graph.json files: `validateGraph` fills `DEFAULT_HARNESS` (serde default).
   */
  harness: string;
}

export interface Position {
  x: number;
  y: number;
}

export interface Deliverable {
  /** Relative to `nodes/<id>/deliverables/`. */
  path: string;
  description: string;
}

export interface Node {
  /** Slug, unique, used as folder name. */
  id: string;
  title: string;
  position: Position;
  /** Free-form task text typed like a Claude Code prompt (simple mode). "" when unset. */
  prompt: string;
  goal: string;
  acceptance: string;
  scope_in: string;
  scope_out: string;
  already_known: string;
  deliverables: Deliverable[];
  do_nots: string[];
  model: Model;
  /** Any of cpu, memory, gpu, exclusive, or free-text tags. */
  resources: string[];
  permission_mode: PermissionMode;
  allowed_tools: string[] | null;
  max_turns: number | null;
  /** null = graph dir. */
  working_dir: string | null;
  /** Upstream node id whose session is resumed. */
  continue_session_of: string | null;
  extra_context: string;
  /** Harness profile id; null = inherit `settings.harness`. */
  harness: string | null;
}

export interface Edge {
  id: string;
  from: string;
  to: string;
}

export interface Graph {
  version: number;
  name: string;
  settings: Settings;
  nodes: Node[];
  edges: Edge[];
}

// ---------------------------------------------------------------------------
// Harness profiles (HARNESS_CONTRACT): which coding-agent CLI a session runs
// ---------------------------------------------------------------------------

/** Id of the default harness (Claude Code). */
export const DEFAULT_HARNESS = 'claude';

/** `deepseek-harness`: the dsh CLI, headless only (one process per message, chained by its session id). */
export const HARNESS_KINDS = ['claude', 'codex', 'custom', 'deepseek-harness'] as const;
/** Picks the adapter: readiness / submit / stop detection and the command shape. */
export type HarnessKind = (typeof HARNESS_KINDS)[number];

export const STOP_MODES = ['quiet', 'claude_hooks', 'codex_notify'] as const;
/** How a custom harness's turn end is detected. */
export type StopMode = (typeof STOP_MODES)[number];

/**
 * One harness profile (`list_harnesses`). Machine-local: stored in app data,
 * never in graph.json. Env values may hold `${VAR}`, read from the app's
 * environment when a session starts; the app stores no secrets.
 */
export interface HarnessProfile {
  /** ^[a-z0-9][a-z0-9-]{0,31}$; built-ins: claude, claude-deepseek, codex, deepseek-harness. */
  id: string;
  label: string;
  kind: HarnessKind;
  /** null = resolve the default program of the kind; else a program path or name. */
  command: string | null;
  /** Extra args appended at session start. */
  args: string[];
  env: Record<string, string>;
  /** Harness-native model passed at start (codex: -m, claude: --model, custom: ignored). */
  model: string | null;
  /** Built-ins cannot be deleted; their command, args, env and model may be edited. */
  builtin: boolean;
  /** custom only: text on screen when the CLI waits for input. */
  idle_marker: string | null;
  /** custom only: texts that mean a dialog is open. */
  dialog_markers: string[];
  /** custom only. */
  stop_mode: StopMode;
  /** custom only, stop_mode quiet: ms of no output after the submit = turn over (default 8000). */
  quiet_ms: number | null;
  /** custom only: regex of a dialog's option rows (groups `label`, optional `num`, `cur`). Missing = null. */
  option_pattern?: string | null;
  /** custom only: extra texts that mark a dialog. Missing = []. */
  dialog_hints?: string[];
  /** deepseek-harness only: the dsh profile (`--profile`); null = headless. */
  dsh_profile?: string | null;
}

/** Return value of `check_harness`. */
export interface HarnessCheck {
  id: string;
  ok: boolean;
  version: string | null;
  message: string | null;
}

/** Graph default harness id; a missing or empty value means `claude`. */
export function harnessOf(settings: { harness?: unknown } | null | undefined): string {
  const h = settings?.harness;
  return typeof h === 'string' && h !== '' ? h : DEFAULT_HARNESS;
}

/** Effective harness of a node: its own, else the graph default. */
export function effectiveHarness(node: { harness?: string | null }, settings: { harness?: unknown } | null | undefined): string {
  return node.harness ?? harnessOf(settings);
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

export type RunTarget = 'all' | { from: string } | { failed_of: string };

export const NODE_STATUSES = [
  'pending',
  'queued',
  'running',
  'succeeded',
  'failed',
  /** The agent ended with `## Status: BLOCKED` (asked the user something); scheduled like failed. */
  'blocked',
  'skipped',
  'cancelled',
  /** Terminal mode: the agent asked something (a question or a dialog); the session waits for you. */
  'waiting',
] as const;
export type NodeStatus = (typeof NODE_STATUSES)[number];

export const RUN_STATUSES = ['running', 'succeeded', 'failed', 'cancelled'] as const;
export type RunStatus = (typeof RUN_STATUSES)[number];

export interface NodeRunState {
  status: NodeStatus;
  started_at: string | null;
  finished_at: string | null;
  session_id: string | null;
  cost_usd: number | null;
  num_turns: number | null;
  /** Why a node failed / was blocked / was skipped / was cancelled. */
  reason: string | null;
  /**
   * Tool names the CLI denied, deduplicated, first-seen order. Rust fills it
   * with serde default; treat a missing value as [] (see `deniedTools`).
   */
  permission_denials?: string[];
}

export const SESSION_STATUSES = ['starting', 'running', 'done', 'failed', 'cancelled'] as const;
/** done = every node was sent; the process stays alive so the user can keep talking. */
export type SessionStatus = (typeof SESSION_STATUSES)[number];

/** One Claude Code session of a terminal-mode run (one weakly connected component). */
export interface TerminalSession {
  /** `s1`, `s2`, … */
  sid: string;
  title: string;
  /** In the order they are sent. */
  node_ids: string[];
  status: SessionStatus;
  pid: number | null;
  current_node: string | null;
  /**
   * Harness profile id the session runs. Missing in older runs (serde default
   * `claude`): read it through `sessionHarness`.
   */
  harness?: string;
  /** Id of the question or dialog the session waits on (`list_session_prompts` has it). */
  pending_prompt?: string | null;
  /**
   * The session takes messages (its process, or for chained sessions its
   * task, is alive). Chained sessions have no pid. Missing in older payloads.
   */
  alive?: boolean;
}

/** Payload of `pty:data`. */
export interface PtyData {
  run_id: string;
  sid: string;
  data: string;
  /** 1-based chunk counter of the session. */
  seq: number;
}

/** Return value of `pty_replay`: the transcript so far and the seq of its last chunk (0 = none). */
export interface PtyReplay {
  text: string;
  seq: number;
}

/** Content of runs/<run-id>/run.json and payload of `run:state`. */
export interface RunState {
  run_id: string;
  graph_dir: string;
  started_at: string;
  finished_at: string | null;
  status: RunStatus;
  /** Keyed by node id. Only nodes in the run's target set appear. */
  nodes: Record<string, NodeRunState>;
  /** Missing in older runs: headless (see `runStateMode`). */
  mode?: RunMode;
  /** Terminal mode only; serde default []. Read through `sessionsOf`. */
  sessions?: TerminalSession[];
}

export type StatusCounts = Record<NodeStatus, number>;

/** One row of `list_runs`. */
export interface RunSummary {
  run_id: string;
  started_at: string;
  finished_at: string | null;
  status: RunStatus;
  counts: StatusCounts;
  /** Sum of cost_usd over nodes that reported one; null if none did. */
  total_cost_usd: number | null;
}

/** Content of runs/<run-id>/<node-id>/summary.json. */
export interface NodeSummary {
  session_id: string | null;
  cost_usd: number | null;
  duration_ms: number | null;
  num_turns: number | null;
  is_error: boolean;
  status: NodeStatus;
  /** Same as NodeRunState.permission_denials; missing = []. */
  permission_denials?: string[];
}

/** Return value of `read_node_result`. */
export interface NodeResult {
  result_md: string | null;
  summary: NodeSummary | null;
  /** Last raw lines of events.jsonl, oldest first. */
  events_tail: string[];
  /** Terminal mode: questions.jsonl records; read through `questionsOf`. */
  questions?: QuestionRecord[];
}

export const NODE_EVENT_KINDS = [
  'text',
  'tool_use',
  'tool_result',
  'system',
  'result',
  'stderr',
  'parse_error',
  'user_message',
  /** A tool call the CLI denied; text = tool name plus a short input summary. */
  'permission_denied',
  /** ask_app: a tool call waits for the user's answer; text = `<tool>: <summary>`. */
  'permission_prompt',
  /** ask_app: the answer; text = `allowed` / `denied` plus the scope. */
  'permission_answered',
  /** Terminal mode: the reply ended with a question; text = the question. */
  'question',
  /** Terminal mode: a dialog waits in the window; text = question and options. */
  'dialog',
  /** Terminal mode: a question or dialog was answered; text = how. */
  'answer',
] as const;
export type NodeEventKind = (typeof NODE_EVENT_KINDS)[number];

/** Payload of `node:event`. */
export interface NodeEvent {
  run_id: string;
  node_id: string;
  kind: NodeEventKind;
  text: string;
  ts: string;
}

/** One chat message of a node session (`list_messages`). */
export interface NodeMessage {
  role: 'user' | 'assistant';
  text: string;
  ts: string;
}

/** Return value of `check_claude`. */
export interface ClaudeCheck {
  found: boolean;
  path: string | null;
  version: string | null;
}

/**
 * Return value of `launch_options`: graph folder from argv / `CLAUDE_GRAPH_OPEN`,
 * and whether to start `run all` once it is open (`--run` / `CLAUDE_GRAPH_AUTORUN`).
 * `autorun` is only true when `open_dir` is set.
 */
export interface LaunchOptions {
  open_dir: string | null;
  autorun: boolean;
}

/** Per-machine app settings (`get_app_settings` / `set_app_settings`). */
export interface AppSettings {
  /** Absolute path of the claude CLI; null = env CLAUDE_GRAPH_CLAUDE_BIN, then PATH. */
  claude_bin: string | null;
}

export const TRUST_ITEM_KINDS = [
  'bypass_permissions',
  'dont_ask',
  'bash_tool',
  'absolute_working_dir',
  'claude_bin_in_graph',
  'shell_prompt',
  'slash_prompt',
  'project_settings',
  'custom_harness',
] as const;
export type TrustItemKind = (typeof TRUST_ITEM_KINDS)[number];

/** One elevated setting of a graph that needs the user's trust before a run. */
export interface TrustItem {
  /** null for graph-level items (default permission mode, claude_bin). */
  node_id: string | null;
  kind: TrustItemKind;
  detail: string;
}

/** Return value of `graph_trust_status`. */
export interface TrustStatus {
  /** true when `items` is empty or their fingerprint was accepted via `trust_graph`. */
  trusted: boolean;
  items: TrustItem[];
}

/**
 * Error returned by every command. Codes in use: not_implemented, io, json,
 * invalid_graph, not_found, claude_not_found, run_active, internal, spawn_failed,
 * invalid_input, untrusted_graph (start_run on a graph whose elevated settings
 * were not accepted via trust_graph). send_message: a node that never ran ->
 * not_found; empty text -> invalid_input. assistant_send / assistant_reset while a
 * scheduler turn runs -> assistant_busy. `interrupted` appears as a node
 * `reason` for runs found stale at startup.
 */
export interface AppError {
  code: string;
  message: string;
}

export function isAppError(value: unknown): value is AppError {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as AppError).code === 'string' &&
    typeof (value as AppError).message === 'string'
  );
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isNullableString = (v: unknown): boolean => v === null || typeof v === 'string';
const isOptionalStringArray = (v: unknown): boolean =>
  v === undefined || (Array.isArray(v) && v.every((x) => typeof x === 'string'));

/** Denied tool names of a node state or summary; [] when missing or malformed. */
export function deniedTools(n: { permission_denials?: unknown } | null | undefined): string[] {
  const v = n?.permission_denials;
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

/** Run mode of graph settings; a missing or unknown value means terminal. */
export function runModeOf(settings: { run_mode?: unknown } | null | undefined): RunMode {
  return settings?.run_mode === 'headless' ? 'headless' : DEFAULT_RUN_MODE;
}

/** Mode of a run; runs recorded before terminal mode existed are headless. */
export function runStateMode(rs: { mode?: unknown } | null | undefined): RunMode {
  return rs?.mode === 'terminal' ? 'terminal' : 'headless';
}

export function isTerminalSession(v: unknown): v is TerminalSession {
  return (
    isObject(v) &&
    typeof v.sid === 'string' &&
    typeof v.title === 'string' &&
    Array.isArray(v.node_ids) &&
    v.node_ids.every((x) => typeof x === 'string') &&
    (SESSION_STATUSES as readonly unknown[]).includes(v.status) &&
    (v.pid === null || v.pid === undefined || typeof v.pid === 'number') &&
    (v.current_node === null || v.current_node === undefined || typeof v.current_node === 'string') &&
    (v.harness === undefined || typeof v.harness === 'string') &&
    (v.pending_prompt === undefined || v.pending_prompt === null || typeof v.pending_prompt === 'string') &&
    (v.alive === undefined || typeof v.alive === 'boolean')
  );
}

/** Well-formed sessions of a run (or of a list_sessions reply); [] when missing. */
export function sessionsOf(rs: { sessions?: unknown } | null | undefined): TerminalSession[] {
  return validSessions(rs?.sessions);
}

export function validSessions(v: unknown): TerminalSession[] {
  if (!Array.isArray(v)) return [];
  return v
    .filter(isTerminalSession)
    .map((s) => ({ ...s, node_ids: [...s.node_ids], pid: s.pid ?? null, current_node: s.current_node ?? null }));
}

/** Harness id of a session; sessions of older runs ran Claude Code. */
export function sessionHarness(s: { harness?: unknown } | null | undefined): string {
  return typeof s?.harness === 'string' && s.harness !== '' ? s.harness : DEFAULT_HARNESS;
}

/** Shape check of a RunState payload (event or command reply) before it enters the store. */
export function isRunState(value: unknown): value is RunState {
  if (!isObject(value)) return false;
  const { run_id, graph_dir, started_at, finished_at, status, nodes } = value;
  if (typeof run_id !== 'string' || typeof graph_dir !== 'string' || typeof started_at !== 'string') return false;
  if (!isNullableString(finished_at) || !(RUN_STATUSES as readonly unknown[]).includes(status) || !isObject(nodes)) return false;
  return Object.values(nodes).every(
    (n) =>
      isObject(n) &&
      (NODE_STATUSES as readonly unknown[]).includes(n.status) &&
      isNullableString(n.reason ?? null) &&
      isOptionalStringArray(n.permission_denials),
  );
}

// ---------------------------------------------------------------------------
// Assistant mode (one long-lived scheduler session per graph folder)
// ---------------------------------------------------------------------------

/** Event: full AssistantState after every change (no dir in the payload: reload). */
export const EVENT_ASSISTANT_UPDATE = 'assistant_update';
/** `run_id` / `node_id` of the scheduler turn's lines on the `node:event` channel. */
export const ASSISTANT_RUN_ID = 'assistant';
export const ASSISTANT_NODE_ID = 'scheduler';

export const ASSISTANT_STATUSES = ['idle', 'working', 'waiting', 'done', 'failed', 'cancelled'] as const;
/**
 * idle: no turn running and nothing pending; working: a turn is running;
 * waiting: the last reply asked a `## Question`; done: the last reply had `## Final`;
 * failed: the last turn errored; cancelled: the last turn was cancelled.
 */
export type AssistantStatus = (typeof ASSISTANT_STATUSES)[number];

/** Content of assistant/session.json. */
export interface AssistantSession {
  /** CLI session id, used for --resume. */
  session_id: string | null;
  status: AssistantStatus;
  /** Completed + running turns (turn folder numbers start at 1). */
  turns: number;
  /** As reported by the CLI init event; display only. */
  model: string | null;
  /** sha256 hex of the rendered workflow last sent. */
  workflow_hash: string | null;
  last_error: string | null;
  updated_at: string;
}

export const ASSISTANT_ROLES = ['user', 'scheduler', 'app'] as const;
export type AssistantRole = (typeof ASSISTANT_ROLES)[number];

export const ASSISTANT_KINDS = [
  'instruction',
  'answer',
  'stage',
  'question',
  'decision',
  'note',
  'final',
  'system',
  'error',
] as const;
/** user: instruction | answer; scheduler: stage | question | decision | note | final; app: system | error. */
export type AssistantKind = (typeof ASSISTANT_KINDS)[number];

/** One line of assistant/journal.jsonl. */
export interface AssistantMessage {
  id: string;
  ts: string;
  /** Turn this message belongs to (0 for app messages outside a turn). */
  turn: number;
  role: AssistantRole;
  kind: AssistantKind;
  /** Stage name or decision title; null otherwise. */
  title: string | null;
  text: string;
}

/** Return value of assistant_state / assistant_send / assistant_reset and payload of `assistant_update`. */
export interface AssistantState {
  session: AssistantSession;
  /** Whole journal, in order. */
  messages: AssistantMessage[];
}

const isAssistantMessage = (m: unknown): boolean =>
  isObject(m) &&
  typeof m.id === 'string' &&
  typeof m.ts === 'string' &&
  typeof m.turn === 'number' &&
  (ASSISTANT_ROLES as readonly unknown[]).includes(m.role) &&
  (ASSISTANT_KINDS as readonly unknown[]).includes(m.kind) &&
  isNullableString(m.title) &&
  typeof m.text === 'string';

/** Shape check of an AssistantState reply before it enters the store. */
export function isAssistantState(value: unknown): value is AssistantState {
  if (!isObject(value) || !isObject(value.session) || !Array.isArray(value.messages)) return false;
  const s = value.session;
  if (!(ASSISTANT_STATUSES as readonly unknown[]).includes(s.status) || typeof s.turns !== 'number') return false;
  if (!isNullableString(s.session_id) || !isNullableString(s.model) || !isNullableString(s.last_error)) return false;
  return value.messages.every(isAssistantMessage);
}

// ---------------------------------------------------------------------------
// "Ask me in the app" permission prompts (ASKAPP_CONTRACT)
// ---------------------------------------------------------------------------

/** Event: one `PermissionPrompt` that waits for the user's answer. */
export const EVENT_PERMISSION_PROMPT = 'permission_prompt';
/** Event: `{ id }` of a prompt that was resolved (by the user, by cancel, or by a remembered scope). */
export const EVENT_PERMISSION_ANSWERED = 'permission_answered';

/** One tool call waiting for approval (`list_permission_prompts`, `permission_prompt`). */
export interface PermissionPrompt {
  id: string;
  /** `assistant` for Assistant-tab turns. */
  run_id: string;
  /** `scheduler` for the assistant. */
  node_id: string;
  /** e.g. Bash, WebSearch, mcp__x__y. */
  tool_name: string;
  /** The tool input as received (JSON). */
  input: unknown;
  /** Backend-made one-liner (command, file path, query, url, or the start of the JSON). */
  summary: string;
  /** Raw permission update suggestions of the CLI; [] when none. */
  suggestions: unknown[];
  ts: string;
}

export const PERMISSION_SCOPES = ['once', 'node', 'graph'] as const;
/** once: this call only; node: until the run ends, for this node; graph: remembered for the graph folder. */
export type PermissionScope = (typeof PERMISSION_SCOPES)[number];

/** Argument of `answer_permission`. */
export interface PermissionAnswer {
  id: string;
  allow: boolean;
  scope: PermissionScope;
  message?: string;
}

/** Payload of `permission_answered`. */
export interface PermissionAnswered {
  id: string;
}

/** Shape check of a PermissionPrompt (event or command reply); `suggestions` may be missing. */
export function isPermissionPrompt(value: unknown): value is PermissionPrompt {
  if (!isObject(value)) return false;
  const { id, run_id, node_id, tool_name, summary, ts, suggestions } = value;
  return (
    typeof id === 'string' &&
    id !== '' &&
    typeof run_id === 'string' &&
    typeof node_id === 'string' &&
    typeof tool_name === 'string' &&
    typeof summary === 'string' &&
    typeof ts === 'string' &&
    (suggestions === undefined || Array.isArray(suggestions))
  );
}

/** Well-formed prompts of a reply, `suggestions` defaulted to []; malformed entries are dropped. */
export function validPermissionPrompts(value: unknown): PermissionPrompt[] {
  if (!Array.isArray(value)) return [];
  return value.filter(isPermissionPrompt).map((p) => ({ ...p, suggestions: Array.isArray(p.suggestions) ? [...p.suggestions] : [] }));
}

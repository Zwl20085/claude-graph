/**
 * IPC contract between the React frontend and the Rust backend.
 *
 * Mirrors `src-tauri/src/contract.rs` one-to-one (docs/ARCHITECTURE.md §2, §6).
 * Field names are snake_case on both sides. Timestamps are RFC 3339 UTC strings
 * (chrono `DateTime<Utc>` on the Rust side). Rust `Option<T>` is `T | null`.
 */

export const GRAPH_VERSION = 1;

/** Event: full RunState whenever any node status changes. */
export const EVENT_RUN_STATE = 'run:state';
/** Event: one summarised stream-json line of a running node. */
export const EVENT_NODE_EVENT = 'node:event';

// ---------------------------------------------------------------------------
// Graph (graph.json)
// ---------------------------------------------------------------------------

export const MODELS = ['sonnet', 'opus', 'haiku', 'inherit'] as const;
/** `inherit` on a node means `settings.default_model`. */
export type Model = (typeof MODELS)[number];

export const PERMISSION_MODES = [
  'claude_settings',
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
 */
export type PermissionMode = (typeof PERMISSION_MODES)[number];

export interface Settings {
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

/** Content of runs/<run-id>/run.json and payload of `run:state`. */
export interface RunState {
  run_id: string;
  graph_dir: string;
  started_at: string;
  finished_at: string | null;
  status: RunStatus;
  /** Keyed by node id. Only nodes in the run's target set appear. */
  nodes: Record<string, NodeRunState>;
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

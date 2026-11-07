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
  'acceptEdits',
  'default',
  'plan',
  'bypassPermissions',
  'inherit',
] as const;
/** `inherit` on a node means `settings.default_permission_mode`. */
export type PermissionMode = (typeof PERMISSION_MODES)[number];

export interface Settings {
  max_parallel: number;
  /** Capacity per resource tag. Unknown tags have capacity 1. */
  resources: Record<string, number>;
  stop_on_failure: boolean;
  default_model: Model;
  default_permission_mode: PermissionMode;
  /** null = find `claude` on PATH. */
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
  /** Why a node failed / was skipped / was cancelled. */
  reason: string | null;
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

/** Return value of `check_claude`. */
export interface ClaudeCheck {
  found: boolean;
  path: string | null;
  version: string | null;
}

/**
 * Error returned by every command. Codes in use: not_implemented, io, json,
 * invalid_graph, not_found, claude_not_found, run_active, internal.
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

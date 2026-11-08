/** Status and model -> visual style mapping (pure; consumed by BriefNode, WireEdge, panels). */
import type { Model, NodeStatus, PermissionMode, RunState, StatusCounts } from '../types/contract';
import { NODE_STATUSES } from '../types/contract';

export const MODEL_COLORS: Record<Model, string> = {
  opus: '#8b5cf6',
  sonnet: '#3b82f6',
  haiku: '#22c55e',
  inherit: '#6b7280',
};

export const STATUS_COLORS: Record<NodeStatus, string> = {
  pending: '#555555',
  queued: '#f59e0b',
  running: '#3b82f6',
  succeeded: '#22c55e',
  failed: '#ef4444',
  blocked: '#f97316',
  skipped: '#777777',
  cancelled: '#b45309',
  waiting: '#a78bfa',
};

/** Border colour of a node card that is not part of the shown run. */
export const IDLE_BORDER = '#3c3c3c';
export const EDGE_IDLE = '#8a8a8a';

export interface RingStyle {
  borderColor: string;
  borderStyle: 'solid' | 'dashed';
  /** Extra CSS class: `cg-pulse` animates the running ring. */
  className: string;
}

export function ringStyle(status: NodeStatus | null): RingStyle {
  if (status === null) return { borderColor: IDLE_BORDER, borderStyle: 'solid', className: '' };
  return {
    borderColor: STATUS_COLORS[status],
    borderStyle: status === 'skipped' ? 'dashed' : 'solid',
    className: status === 'running' ? 'cg-pulse' : status === 'waiting' ? 'cg-pulse-wait' : '',
  };
}

export interface EdgeVisual {
  stroke: string;
  /** Animated dash + glow while the source runs. */
  animated: boolean;
  glow: string | null;
}

/** Compile-time exhaustiveness check for switches over closed unions. */
export function assertNever(value: never): never {
  throw new Error(`unexpected value: ${String(value)}`);
}

/** Edge look depends only on the status of its source node. */
export function edgeVisual(sourceStatus: NodeStatus | null): EdgeVisual {
  if (sourceStatus === null) return { stroke: EDGE_IDLE, animated: false, glow: null };
  switch (sourceStatus) {
    case 'running':
      return { stroke: STATUS_COLORS.running, animated: true, glow: STATUS_COLORS.running };
    case 'succeeded':
    case 'failed':
    case 'blocked':
      return { stroke: STATUS_COLORS[sourceStatus], animated: false, glow: null };
    case 'waiting':
    case 'pending':
    case 'queued':
    case 'skipped':
    case 'cancelled':
      return { stroke: EDGE_IDLE, animated: false, glow: null };
    default:
      return assertNever(sourceStatus);
  }
}

/** Label of the `waiting` status: the node asked the user something. */
export const WAITING_LABEL = 'needs you';

/** Display label of a node status (the card, the inspector). */
export function statusLabel(status: NodeStatus): string {
  switch (status) {
    case 'pending':
    case 'queued':
    case 'running':
    case 'succeeded':
    case 'failed':
    case 'blocked':
    case 'skipped':
    case 'cancelled':
      return status;
    case 'waiting':
      return WAITING_LABEL;
    default:
      return assertNever(status);
  }
}

/** Label of a permission mode in the selects (graph default and per node). */
export function permissionLabel(mode: PermissionMode): string {
  switch (mode) {
    case 'claude_settings':
      return 'Claude Code settings (recommended)';
    case 'ask_app':
      return 'Ask me in the app';
    case 'acceptEdits':
    case 'default':
    case 'plan':
    case 'dontAsk':
    case 'bypassPermissions':
      return mode;
    case 'inherit':
      return 'inherit';
    default:
      return assertNever(mode);
  }
}

/** Hint under a permission select set to ask_app. */
export const ASK_APP_HINT = 'Tool calls that need approval open a dialog in this app (headless engine).';

/** Hint shown with denied tools (toast, Results tab). */
export const DENIED_HINT = 'Set its permission mode to "Claude Code settings" or add the tool to its allowed tools.';
export const RERUN_FAILED_TITLE = 'Rerun the failed, blocked, skipped and cancelled nodes of the shown run, and their dependents';

export function emptyCounts(): StatusCounts {
  return Object.fromEntries(NODE_STATUSES.map((s) => [s, 0])) as StatusCounts;
}

export function countStatuses(run: RunState): StatusCounts {
  return Object.values(run.nodes).reduce<StatusCounts>(
    (acc, n) => ({ ...acc, [n.status]: acc[n.status] + 1 }),
    emptyCounts(),
  );
}

export function isRunActive(run: RunState | null): boolean {
  return run !== null && run.status === 'running';
}

/**
 * "Rerun failed" is offered only for a finished run with at least one failed,
 * blocked, skipped or cancelled node (the backend reruns those and their dependents).
 */
export function runHasFailures(run: RunState | null): boolean {
  if (run === null || isRunActive(run)) return false;
  return Object.values(run.nodes).some((n) => n.status !== 'succeeded');
}

export function formatCost(usd: number | null): string {
  if (usd === null) return '—';
  return `$${usd.toFixed(usd < 1 ? 4 : 2)}`;
}

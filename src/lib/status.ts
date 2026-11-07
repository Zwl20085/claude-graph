/** Status and model -> visual style mapping (pure; consumed by BriefNode, WireEdge, panels). */
import type { Model, NodeStatus, RunState, StatusCounts } from '../types/contract';
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
  skipped: '#777777',
  cancelled: '#b45309',
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
    className: status === 'running' ? 'cg-pulse' : '',
  };
}

export interface EdgeVisual {
  stroke: string;
  /** Animated dash + glow while the source runs. */
  animated: boolean;
  glow: string | null;
}

/** Edge look depends only on the status of its source node. */
export function edgeVisual(sourceStatus: NodeStatus | null): EdgeVisual {
  switch (sourceStatus) {
    case 'running':
      return { stroke: STATUS_COLORS.running, animated: true, glow: STATUS_COLORS.running };
    case 'succeeded':
      return { stroke: STATUS_COLORS.succeeded, animated: false, glow: null };
    case 'failed':
      return { stroke: STATUS_COLORS.failed, animated: false, glow: null };
    default:
      return { stroke: EDGE_IDLE, animated: false, glow: null };
  }
}

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
 * skipped or cancelled node (the backend reruns those and their dependents).
 */
export function runHasFailures(run: RunState | null): boolean {
  if (run === null || isRunActive(run)) return false;
  return Object.values(run.nodes).some((n) => n.status !== 'succeeded');
}

export function formatCost(usd: number | null): string {
  if (usd === null) return '—';
  return `$${usd.toFixed(usd < 1 ? 4 : 2)}`;
}

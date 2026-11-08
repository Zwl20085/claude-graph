/**
 * What changed between two states of the shown run that the user should hear
 * about once: the run ended (one toast with counts and cost, "Show" selects
 * the first node that did not succeed), or a session window closed while the
 * run was still active (the chain stopped at a node). Pure functions.
 */
import { sessionsOf, type Graph, type NodeStatus, type RunState } from '../types/contract';
import { countStatuses, formatCost, isRunActive } from '../lib/status';
import { isSessionAlive } from '../lib/sessions';

export interface RunNotice {
  kind: 'success' | 'warning' | 'error';
  text: string;
  nodeId?: string;
}

/** Title of a node in `graph`, else its id. */
export function titleOf(graph: Graph | null, nodeId: string): string {
  const t = graph?.nodes.find((n) => n.id === nodeId)?.title?.trim();
  return t ? t : nodeId;
}

const NOT_DONE: readonly NodeStatus[] = ['failed', 'blocked', 'cancelled', 'skipped'];

/** "3 succeeded, 1 failed" (non-zero counts, in status order). */
export function countsText(run: RunState): string {
  const counts = countStatuses(run);
  const parts = Object.entries(counts)
    .filter(([, n]) => n > 0)
    .map(([s, n]) => `${n} ${s === 'waiting' ? 'waiting' : s}`);
  return parts.length > 0 ? parts.join(', ') : 'no nodes';
}

export function runCost(run: RunState): number | null {
  const costs = Object.values(run.nodes)
    .map((n) => n.cost_usd)
    .filter((c): c is number => typeof c === 'number');
  return costs.length === 0 ? null : costs.reduce((a, b) => a + b, 0);
}

/** The first node (graph order) of `run` that did not succeed, or null. */
export function firstProblem(run: RunState, graph: Graph | null): string | null {
  const order = graph?.nodes.map((n) => n.id) ?? Object.keys(run.nodes);
  return order.find((id) => NOT_DONE.includes(run.nodes[id]?.status ?? 'pending')) ?? null;
}

/** The run ended between `prev` and `next` (same run). */
export function runEndNotice(prev: RunState | null, next: RunState, graph: Graph | null): RunNotice | null {
  if (prev === null || prev.run_id !== next.run_id || !isRunActive(prev) || isRunActive(next)) return null;
  const problem = firstProblem(next, graph);
  const cost = runCost(next);
  const text = `Run ${next.status}: ${countsText(next)}${cost === null ? '' : ` — ${formatCost(cost)}`}`;
  const kind = next.status === 'succeeded' ? 'success' : next.status === 'cancelled' ? 'warning' : 'error';
  return problem === null ? { kind, text } : { kind, text, nodeId: problem };
}

/** Sessions that stopped taking messages while the run was active. */
export function closedSessionNotices(prev: RunState | null, next: RunState, graph: Graph | null): RunNotice[] {
  if (prev === null || prev.run_id !== next.run_id || !isRunActive(prev)) return [];
  const before = sessionsOf(prev);
  return sessionsOf(next)
    .filter((s) => {
      const old = before.find((b) => b.sid === s.sid);
      return old !== undefined && isSessionAlive(old) && !isSessionAlive(s) && s.status !== 'done';
    })
    .map((s) => {
      const stopped = s.node_ids.find((id) => NOT_DONE.includes(next.nodes[id]?.status ?? 'pending')) ?? s.node_ids[0] ?? '';
      const text = `Session “${s.title || s.sid}” closed — the chain stopped at ${titleOf(graph, stopped)}`;
      return { kind: 'warning' as const, text, nodeId: stopped };
    });
}

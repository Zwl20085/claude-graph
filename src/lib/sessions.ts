/**
 * Pure helpers of terminal mode: session grouping (mirrors the backend rule in
 * TERMINAL_CONTRACT "Sessions"), session liveness, and the colours of a
 * session chip. No store, no IPC: the terminal window imports this too.
 */
import type { Graph, Node, NodeRunState, NodeStatus, SessionStatus, TerminalSession } from '../types/contract';

export const SESSION_TITLE_MAX = 60;

export const SESSION_COLORS: Record<SessionStatus, string> = {
  starting: '#f59e0b',
  running: '#3b82f6',
  done: '#22c55e',
  failed: '#ef4444',
  cancelled: '#b45309',
};

/**
 * A session whose Claude Code process is still there to type into: starting
 * or running, or done while its process lives (`pid` is null once it is gone).
 */
export function isSessionAlive(s: Pick<TerminalSession, 'status' | 'pid'> | null | undefined): boolean {
  if (s === null || s === undefined) return false;
  return s.status === 'starting' || s.status === 'running' || (s.status === 'done' && s.pid !== null);
}

/** The session that contains `nodeId`, or null. */
export function sessionOfNode(sessions: readonly TerminalSession[], nodeId: string): TerminalSession | null {
  return sessions.find((s) => s.node_ids.includes(nodeId)) ?? null;
}

const byPosition = (a: Node, b: Node): number => a.position.y - b.position.y || a.position.x - b.position.x;

/** Kahn's order of `nodes` over `edges`; ready ties broken by position (y, then x). */
function topoOrder(nodes: readonly Node[], edges: ReadonlyArray<{ from: string; to: string }>): Node[] {
  const indegree = new Map(nodes.map((n) => [n.id, 0]));
  for (const e of edges) indegree.set(e.to, (indegree.get(e.to) ?? 0) + 1);
  const out: Node[] = [];
  let ready = nodes.filter((n) => indegree.get(n.id) === 0);
  while (ready.length > 0) {
    const [next, ...rest] = [...ready].sort(byPosition);
    if (!next) break;
    out.push(next);
    const released = edges
      .filter((e) => e.from === next.id)
      .map((e) => {
        const left = (indegree.get(e.to) ?? 1) - 1;
        indegree.set(e.to, left);
        return left === 0 ? nodes.find((n) => n.id === e.to) : undefined;
      })
      .filter((n): n is Node => n !== undefined);
    ready = [...rest, ...released];
  }
  return out;
}

function sessionTitle(roots: readonly Node[]): string {
  const text = roots.map((n) => n.title.trim() || n.id).join(' + ');
  return text.length > SESSION_TITLE_MAX ? text.slice(0, SESSION_TITLE_MAX) : text;
}

/**
 * Sessions of a run over `ids`: weakly connected components of the induced
 * subgraph, numbered s1, s2, … by their top-left node; nodes in send order.
 */
export function groupSessions(graph: Graph, ids: readonly string[]): TerminalSession[] {
  const inRun = new Set(ids);
  const nodes = graph.nodes.filter((n) => inRun.has(n.id));
  const edges = graph.edges.filter((e) => inRun.has(e.from) && inRun.has(e.to));
  const component = new Map<string, number>();
  let count = 0;
  for (const start of nodes) {
    if (component.has(start.id)) continue;
    const stack = [start.id];
    component.set(start.id, count);
    while (stack.length > 0) {
      const id = stack.pop() as string;
      for (const e of edges) {
        const other = e.from === id ? e.to : e.to === id ? e.from : null;
        if (other !== null && !component.has(other)) {
          component.set(other, count);
          stack.push(other);
        }
      }
    }
    count += 1;
  }
  const groups = Array.from({ length: count }, (_, i) => nodes.filter((n) => component.get(n.id) === i));
  return groups
    .map((members) => ({ members, first: [...members].sort(byPosition)[0] as Node }))
    .sort((a, b) => byPosition(a.first, b.first))
    .map(({ members }, i) => {
      const ids = new Set(members.map((n) => n.id));
      const inner = edges.filter((e) => ids.has(e.from) && ids.has(e.to));
      const ordered = topoOrder(members, inner);
      const roots = ordered.filter((n) => !inner.some((e) => e.to === n.id));
      return {
        sid: `s${i + 1}`,
        title: sessionTitle(roots),
        node_ids: ordered.map((n) => n.id),
        status: 'starting' as const,
        pid: null,
        current_node: null,
      };
    });
}

const FINISHED: ReadonlySet<NodeStatus> = new Set(['succeeded', 'failed', 'blocked', 'skipped', 'cancelled']);

/**
 * Session status derived from its nodes (used by the mock; the backend tracks
 * the real process). `cancelled` once the run was cancelled before it ended.
 */
export function deriveSession(
  session: TerminalSession,
  nodes: Readonly<Record<string, NodeRunState>>,
  runCancelled: boolean,
  pid: number,
): TerminalSession {
  const states = session.node_ids.map((id) => nodes[id]).filter((n): n is NodeRunState => n !== undefined);
  const running = session.node_ids.find((id) => nodes[id]?.status === 'running') ?? null;
  const started = states.some((n) => n.started_at !== null || FINISHED.has(n.status));
  const allDone = states.every((n) => FINISHED.has(n.status));
  const status: SessionStatus = running
    ? 'running'
    : allDone
      ? runCancelled && states.some((n) => n.status === 'cancelled')
        ? 'cancelled'
        : 'done'
      : started
        ? 'running'
        : 'starting';
  return { ...session, status, current_node: running, pid: started ? pid : session.pid };
}

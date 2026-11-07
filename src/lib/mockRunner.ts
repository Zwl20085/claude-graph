/**
 * Timer-driven run simulator behind the mock IPC. Mirrors the scheduler rules
 * of ARCHITECTURE.md §4 closely enough for UI work: target sets, readiness,
 * max_parallel, failure -> dependents skipped, stop_on_failure, cancel.
 * Every emitted RunState is a fresh object.
 */
import type {
  AppError,
  Graph,
  Node,
  NodeEventKind,
  NodeMessage,
  NodeResult,
  NodeRunState,
  NodeStatus,
  RunState,
  RunTarget,
} from '../types/contract';
import { runModeOf } from '../types/contract';
import type { Timers } from './debounce';
import type { AskDecision, AskRequest } from './mockPermissions';
import { deriveSession, groupSessions } from './sessions';
import { targetSet } from './graphOps';
import { assertNever } from './status';

export { targetSet };

export interface MockRun {
  state: RunState;
  graph: Graph;
  results: Record<string, NodeResult>;
  messages: Record<string, NodeMessage[]>;
}

/** Delay of the fake assistant reply to send_message. */
export const MOCK_REPLY_MS = 1500;
/** Delay between a follow-up's `result` event and its reply landing in the history. */
export const MOCK_RECORD_MS = 200;

export interface RunnerDeps {
  timers: Timers;
  stepMs: number;
  now: () => Date;
  emitState: (state: RunState) => void;
  emitEvent: (dir: string, runId: string, nodeId: string, kind: NodeEventKind, text: string) => void;
  /** Harness label for session titles (default: the id). */
  harnessLabel?: (id: string) => string;
  /** Approval relay for FAKE_ASK (ASKAPP_CONTRACT); without it FAKE_ASK is ignored. */
  permissions?: MockPermissionHooks;
}

export interface MockPermissionHooks {
  ask: (req: AskRequest, onDecision: (d: AskDecision) => void) => void;
  cancel: (runId: string, nodeId?: string) => void;
  endRun: (runId: string) => void;
}

const TERMINAL: ReadonlySet<NodeStatus> = new Set(['succeeded', 'failed', 'blocked', 'skipped', 'cancelled']);
const BAD: ReadonlySet<NodeStatus> = new Set(['failed', 'blocked', 'skipped', 'cancelled']);
/** Statuses that make the run `failed` and trigger stop_on_failure. */
const FAILING: ReadonlySet<NodeStatus> = new Set(['failed', 'blocked']);

/** Prompt / extra_context markers, like scripts/fake-claude.mjs. */
export const FAKE_BLOCKED = 'FAKE_BLOCKED';
export const FAKE_DENY = 'FAKE_DENY';
export const MOCK_DENIED_TOOL = 'WebSearch';
export const MOCK_BLOCKED_QUESTION = 'Which dataset should I use? (mock)';
/** `FAKE_ASK=<tool>`: the node asks for approval of one call of <tool> and waits for the answer. */
export const FAKE_ASK_RE = /FAKE_ASK=([A-Za-z0-9_]+)/;
/** Input of the asked call, like scripts/fake-claude.mjs. */
export const MOCK_ASK_INPUT = { command: 'echo hi' } as const;
export const ASK_ALLOWED_FILE = 'asked-allowed.txt';
export const ASK_DENIED_FILE = 'asked-denied.txt';
const hasMarker = (node: Node, marker: string): boolean =>
  node.prompt.includes(marker) || node.extra_context.includes(marker);

/** Tool named by FAKE_ASK=<tool> in the prompt or extra context; null when absent. */
export function askedTool(node: Pick<Node, 'prompt' | 'extra_context'>): string | null {
  const m = FAKE_ASK_RE.exec(node.prompt) ?? FAKE_ASK_RE.exec(node.extra_context);
  return m?.[1] ?? null;
}

type Outcome = 'succeeded' | 'failed' | 'blocked';
type Step = [NodeEventKind, string];

function outcomeOf(node: Node): Outcome {
  if (hasMarker(node, 'FAKE_FAIL')) return 'failed';
  return hasMarker(node, FAKE_BLOCKED) ? 'blocked' : 'succeeded';
}

/** result.md text; null for a failed node (the fake CLI exits 1 with an error result, no reply). */
function resultMdFor(node: Node, outcome: Outcome): string | null {
  switch (outcome) {
    case 'failed':
      return null;
    case 'blocked':
      return `I could not finish without your input.\n\n## Status: BLOCKED\n${MOCK_BLOCKED_QUESTION}`;
    case 'succeeded':
      return `## Deliverables\n${node.deliverables.map((d) => `- ${d.path} — stub`).join('\n')}\n## Status: DONE`;
    default:
      return assertNever(outcome);
  }
}

function reasonFor(outcome: Outcome): string | null {
  switch (outcome) {
    case 'failed':
      return 'claude exited with code 1 (mock FAKE_FAIL)';
    case 'blocked':
      return MOCK_BLOCKED_QUESTION;
    case 'succeeded':
      return null;
    default:
      return assertNever(outcome);
  }
}

/** Fake pid of a mock terminal session (display only). */
export const MOCK_PID_BASE = 4000;

/** Run id = UTC timestamp, e.g. 20261107T101500Z. */
export function runIdFor(date: Date): string {
  return date.toISOString().replace(/[-:]/g, '').replace(/\.\d+Z$/, 'Z');
}

const blankNode = (): NodeRunState => ({
  status: 'pending',
  started_at: null,
  finished_at: null,
  session_id: null,
  cost_usd: null,
  num_turns: null,
  reason: null,
  permission_denials: [],
});

function script(node: Node): Array<[NodeEventKind, string]> {
  const first = node.deliverables[0]?.path ?? 'notes.md';
  const denied: Array<[NodeEventKind, string]> = hasMarker(node, FAKE_DENY)
    ? [
        ['tool_use', `${MOCK_DENIED_TOOL} "mock query"`],
        ['permission_denied', `${MOCK_DENIED_TOOL}: "mock query"`],
      ]
    : [];
  return [
    ['system', `init session (mock) model=${node.model}`],
    ['text', `Reading the brief for "${node.title || node.id}".`],
    ...denied,
    ['tool_use', `Write ${first}`],
    ['tool_result', `wrote ${first}`],
    ['text', 'All deliverables written.'],
  ];
}

export function createMockRunner(deps: RunnerDeps) {
  const runs = new Map<string, MockRun>();
  const handles = new Map<string, unknown[]>();

  const key = (runId: string, nodeId: string): string => `${runId}\u0000${nodeId}`;
  const stamp = (): string => deps.now().toISOString();

  /** Terminal mode: refresh the session statuses from the node statuses, then emit. */
  const emit = (run: MockRun): void => {
    if (run.state.mode === 'terminal') {
      const cancelled = run.state.status === 'cancelled';
      const sessions = (run.state.sessions ?? []).map((s, i) => deriveSession(s, run.state.nodes, cancelled, MOCK_PID_BASE + i));
      run.state = { ...run.state, sessions };
    }
    deps.emitState(run.state);
  };

  const setNode = (run: MockRun, id: string, patch: Partial<NodeRunState>): void => {
    const current = run.state.nodes[id] ?? blankNode();
    run.state = { ...run.state, nodes: { ...run.state.nodes, [id]: { ...current, ...patch } } };
  };

  const clearTimers = (runId: string, nodeId: string): void => {
    for (const h of handles.get(key(runId, nodeId)) ?? []) deps.timers.clearTimeout(h);
    handles.delete(key(runId, nodeId));
  };

  const later = (run: MockRun, nodeId: string, ms: number, fn: () => void): void => {
    const k = key(run.state.run_id, nodeId);
    handles.set(k, [...(handles.get(k) ?? []), deps.timers.setTimeout(fn, ms)]);
  };

  const finishNode = (run: MockRun, node: Node): void => {
    clearTimers(run.state.run_id, node.id);
    const status = outcomeOf(node);
    const failed = status === 'failed';
    const denials = hasMarker(node, FAKE_DENY) ? [MOCK_DENIED_TOOL] : [];
    const cost = Number((0.01 + node.deliverables.length * 0.005).toFixed(4));
    const sessionId = `mock-${run.state.run_id}-${node.id}`;
    const resultMd = resultMdFor(node, status);
    deps.emitEvent(run.state.graph_dir, run.state.run_id, node.id, 'result', `${status} · $${cost} · 3 turns`);
    setNode(run, node.id, {
      status,
      finished_at: stamp(),
      session_id: sessionId,
      cost_usd: cost,
      num_turns: 3,
      reason: reasonFor(status),
      permission_denials: denials,
    });
    run.messages = {
      ...run.messages,
      [node.id]: [
        ...(run.messages[node.id] ?? []),
        { role: 'user', text: node.prompt || node.goal || node.title, ts: run.state.nodes[node.id]?.started_at ?? stamp() },
        ...(resultMd === null ? [] : [{ role: 'assistant' as const, text: resultMd, ts: stamp() }]),
      ],
    };
    run.results = {
      ...run.results,
      [node.id]: {
        result_md: resultMd,
        summary: {
          session_id: sessionId,
          cost_usd: cost,
          duration_ms: deps.stepMs * 6,
          num_turns: 3,
          is_error: failed,
          status,
          permission_denials: denials,
        },
        events_tail: [`{"type":"result","subtype":"${failed ? 'error' : 'success'}"}`],
      },
    };
    advance(run);
  };

  /** Emit `steps` one per stepMs, then call `then` one step later. */
  const play = (run: MockRun, node: Node, steps: readonly Step[], then: () => void): void => {
    steps.forEach(([kind, text], i) => {
      later(run, node.id, deps.stepMs * (i + 1), () =>
        deps.emitEvent(run.state.graph_dir, run.state.run_id, node.id, kind, text),
      );
    });
    later(run, node.id, deps.stepMs * (steps.length + 1), then);
  };

  const stillRunning = (run: MockRun, nodeId: string): boolean =>
    run.state.status === 'running' && run.state.nodes[nodeId]?.status === 'running';

  /** FAKE_ASK: play the head of the script, wait for the answer, then the rest. */
  const playWithAsk = (run: MockRun, node: Node, tool: string, hooks: MockPermissionHooks): void => {
    const steps = script(node);
    const head: Step[] = [...steps.slice(0, 2), ['tool_use', `${tool} ${MOCK_ASK_INPUT.command}`]];
    play(run, node, head, () => {
      const req: AskRequest = {
        graph_dir: run.state.graph_dir,
        run_id: run.state.run_id,
        node_id: node.id,
        tool_name: tool,
        input: { ...MOCK_ASK_INPUT },
      };
      hooks.ask(req, (d) => {
        if (!stillRunning(run, node.id)) return;
        const outcome: Step = d.allow
          ? ['tool_result', `wrote ${ASK_ALLOWED_FILE}`]
          : ['tool_result', `wrote ${ASK_DENIED_FILE}: ${d.message ?? ''}`];
        play(run, node, [outcome, ...steps.slice(2)], () => finishNode(run, node));
      });
    });
  };

  const startNode = (run: MockRun, node: Node): void => {
    setNode(run, node.id, { status: 'running', started_at: stamp() });
    const tool = askedTool(node);
    if (tool !== null && deps.permissions && run.state.mode !== 'terminal') {
      playWithAsk(run, node, tool, deps.permissions);
      return;
    }
    play(run, node, script(node), () => finishNode(run, node));
  };

  /** Settle pending nodes, start what may start, finish the run when nothing is left. */
  function advance(run: MockRun): void {
    if (run.state.status !== 'running') return;
    const inRun = (id: string): boolean => id in run.state.nodes;
    const status = (id: string): NodeStatus => run.state.nodes[id]?.status ?? 'pending';
    const anyFailed = Object.values(run.state.nodes).some((n) => FAILING.has(n.status));
    const halt = anyFailed && run.graph.settings.stop_on_failure;

    let settled = false;
    let changed = true;
    while (changed) {
      changed = false;
      for (const id of Object.keys(run.state.nodes)) {
        if (status(id) !== 'pending') continue;
        const ups = run.graph.edges.filter((e) => e.to === id && inRun(e.from)).map((e) => e.from);
        const bad = ups.find((u) => BAD.has(status(u)));
        if (bad !== undefined || halt) {
          setNode(run, id, { status: 'skipped', reason: halt ? 'stop_on_failure' : `upstream ${bad} ${status(bad as string)}` });
          changed = true;
        } else if (ups.every((u) => status(u) === 'succeeded')) {
          setNode(run, id, { status: 'queued' });
          changed = true;
        }
        settled = settled || changed;
      }
    }
    // Show the settled (queued/skipped) state before anything starts.
    if (settled) emit(run);

    const running = Object.values(run.state.nodes).filter((n) => n.status === 'running').length;
    const free = Math.max(0, run.graph.settings.max_parallel - running);
    const queued = Object.keys(run.state.nodes).filter((id) => status(id) === 'queued').sort();
    if (halt) queued.forEach((id) => setNode(run, id, { status: 'skipped', reason: 'stop_on_failure' }));
    else {
      queued.slice(0, free).forEach((id) => {
        const node = run.graph.nodes.find((n) => n.id === id);
        if (node) startNode(run, node);
      });
    }

    const nodes = Object.values(run.state.nodes);
    if (nodes.every((n) => TERMINAL.has(n.status))) {
      const final = nodes.some((n) => FAILING.has(n.status))
        ? 'failed'
        : nodes.some((n) => n.status === 'cancelled')
          ? 'cancelled'
          : 'succeeded';
      run.state = { ...run.state, status: final, finished_at: stamp() };
      deps.permissions?.endRun(run.state.run_id);
    }
    emit(run);
  }

  const active = (): MockRun | undefined => [...runs.values()].find((r) => r.state.status === 'running');

  return {
    get: (runId: string): MockRun | undefined => runs.get(runId),
    list: (dir: string): MockRun[] => [...runs.values()].filter((r) => r.state.graph_dir === dir),
    start(dir: string, graph: Graph, target: RunTarget): string {
      if (active()) throw { code: 'run_active', message: 'a run is already active' } as AppError;
      const ids = targetSet(graph, target, (id) => runs.get(id)?.state);
      const base = runIdFor(deps.now());
      let runId = base;
      for (let i = 2; runs.has(runId); i += 1) runId = `${base}-${i}`;
      const mode = runModeOf(graph.settings);
      const run: MockRun = {
        graph,
        results: {},
        messages: {},
        state: {
          run_id: runId,
          graph_dir: dir,
          started_at: stamp(),
          finished_at: null,
          status: 'running',
          nodes: Object.fromEntries(ids.map((id) => [id, blankNode()])),
          mode,
          sessions: mode === 'terminal' ? groupSessions(graph, ids, deps.harnessLabel) : [],
        },
      };
      runs.set(runId, run);
      emit(run);
      deps.timers.setTimeout(() => advance(run), 0);
      return runId;
    },
    cancel(runId: string): void {
      const run = runs.get(runId);
      if (!run || run.state.status !== 'running') return;
      for (const [id, n] of Object.entries(run.state.nodes)) {
        clearTimers(runId, id);
        if (!TERMINAL.has(n.status)) setNode(run, id, { status: 'cancelled', finished_at: stamp(), reason: 'run cancelled' });
      }
      run.state = { ...run.state, status: 'cancelled', finished_at: stamp() };
      deps.permissions?.cancel(runId);
      deps.permissions?.endRun(runId);
      emit(run);
    },
    /** Queue a follow-up; a finished node resumes (running) until the fake reply lands. */
    sendMessage(runId: string, nodeId: string, text: string): void {
      const run = runs.get(runId);
      const n = run?.state.nodes[nodeId];
      if (!run || !n) throw { code: 'not_found', message: `node ${nodeId} not in run ${runId}` } as AppError;
      if (n.status === 'pending' || n.status === 'queued' || n.status === 'skipped') {
        throw { code: 'not_found', message: 'node has not run yet; run the node first' } as AppError;
      }
      const add = (m: NodeMessage): void => {
        run.messages = { ...run.messages, [nodeId]: [...(run.messages[nodeId] ?? []), m] };
      };
      add({ role: 'user', text, ts: stamp() });
      deps.emitEvent(run.state.graph_dir, runId, nodeId, 'user_message', text);
      const resumed = n.status !== 'running';
      const before = n.status;
      if (resumed) {
        setNode(run, nodeId, { status: 'running' });
        run.state = { ...run.state, status: 'running', finished_at: null };
        emit(run);
      }
      later(run, nodeId, MOCK_REPLY_MS, () => {
        // Like followup_status: a blocked node's DONE reply promotes it to succeeded.
        const reply = before === 'blocked' ? `(mock) Received: ${text}\n\n## Status: DONE` : `(mock) Received: ${text}`;
        // Like the backend: `result` comes from the stream, the reply is
        // recorded after the process exits, then run:state follows.
        deps.emitEvent(run.state.graph_dir, runId, nodeId, 'result', reply);
        later(run, nodeId, MOCK_RECORD_MS, () => {
          add({ role: 'assistant', text: reply, ts: stamp() });
          const turns = (run.state.nodes[nodeId]?.num_turns ?? 0) + 1;
          if (resumed) {
            const after: NodeStatus = before === 'cancelled' || before === 'blocked' ? 'succeeded' : before;
            const reason = after === 'succeeded' ? null : (run.state.nodes[nodeId]?.reason ?? null);
            setNode(run, nodeId, { status: after, finished_at: stamp(), num_turns: turns, reason });
            advance(run);
          } else {
            setNode(run, nodeId, { num_turns: turns });
            emit(run);
          }
        });
      });
    },
    cancelNode(runId: string, nodeId: string): void {
      const run = runs.get(runId);
      const n = run?.state.nodes[nodeId];
      if (!run || !n || TERMINAL.has(n.status)) return;
      clearTimers(runId, nodeId);
      setNode(run, nodeId, { status: 'cancelled', finished_at: stamp(), reason: 'cancelled by user' });
      deps.permissions?.cancel(runId, nodeId);
      advance(run);
    },
  };
}

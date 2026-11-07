/**
 * Timer-driven stand-in for the assistant-mode work scheduler (mock mode).
 * Script: the first instruction of a session replies with a `stage` (first
 * root node, or "plan") and a `question` (status waiting); an answer replies
 * with a `stage` and a `final` (done); any later instruction replies with a
 * `note` and a `final`. Text containing FAKE_FAIL fails the turn. While a turn
 * runs, `node:event` lines with run_id "assistant" / node_id "scheduler" are
 * emitted for the log panel. Every emitted state is a fresh object.
 */
import {
  ASSISTANT_NODE_ID,
  ASSISTANT_RUN_ID,
  type AppError,
  type AssistantKind,
  type AssistantMessage,
  type AssistantRole,
  type AssistantState,
  type Graph,
  type NodeEventKind,
} from '../types/contract';
import type { Timers } from './debounce';

/** Delay between the send and the scheduler's reply. */
export const MOCK_ASSISTANT_REPLY_MS = 600;
export const MOCK_ASSISTANT_MODEL = 'mock-model';
export const MOCK_SESSION_RESET_TEXT = 'Session reset: the next instruction starts a fresh scheduler session';

export interface AssistantDeps {
  timers: Timers;
  now: () => Date;
  replyMs: number;
  /** The graph of a folder (throws not_found when there is none). */
  graphOf: (dir: string) => Graph;
  emitUpdate: (state: AssistantState) => void;
  emitEvent: (runId: string, nodeId: string, kind: NodeEventKind, text: string) => void;
}

type Phase = 'fresh' | 'asked' | 'finished';

interface Entry {
  state: AssistantState;
  phase: Phase;
  handles: unknown[];
}

type Reply = ReadonlyArray<readonly [AssistantKind, string | null, string]>;

const appError = (code: string, message: string): AppError => ({ code, message });
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

function rootIds(graph: Graph): string[] {
  const hasUpstream = new Set(graph.edges.map((e) => e.to));
  return graph.nodes.filter((n) => !hasUpstream.has(n.id)).map((n) => n.id);
}

function sinkIds(graph: Graph): string[] {
  const hasDownstream = new Set(graph.edges.map((e) => e.from));
  return graph.nodes.filter((n) => !hasDownstream.has(n.id)).map((n) => n.id);
}

function replyFor(graph: Graph, phase: Phase, kind: AssistantKind): { reply: Reply; next: Phase } {
  const first = rootIds(graph)[0] ?? 'plan';
  const last = sinkIds(graph).slice(-1)[0] ?? 'plan';
  if (kind === 'answer') {
    return {
      next: 'finished',
      reply: [
        ['stage', last, `Node ${last} finished (mock). Files are in nodes/${last}/deliverables/.`],
        ['final', null, 'The workflow is complete (mock). See assistant/PLAN.md for the node status.'],
      ],
    };
  }
  if (phase === 'fresh') {
    return {
      next: 'asked',
      reply: [
        ['stage', first, `Node ${first} finished (mock). Wrote nodes/${first}/RESULT.md.`],
        ['question', null, 'Should the remaining nodes run in parallel where the resources allow? (yes / no)'],
      ],
    };
  }
  return {
    next: 'finished',
    reply: [
      ['note', null, 'Treating this as new work in the same project (mock).'],
      ['final', null, 'Done (mock). Nothing else is pending.'],
    ],
  };
}

const SCRIPT: ReadonlyArray<readonly [NodeEventKind, string]> = [
  ['system', `init session (mock) model=${MOCK_ASSISTANT_MODEL}`],
  ['text', 'Reading the workflow and writing the plan.'],
  ['tool_use', 'Write assistant/PLAN.md'],
  ['tool_result', 'wrote assistant/PLAN.md'],
];

export function emptyAssistantState(updatedAt: string): AssistantState {
  return {
    session: { session_id: null, status: 'idle', turns: 0, model: null, workflow_hash: null, last_error: null, updated_at: updatedAt },
    messages: [],
  };
}

export function createMockAssistant(deps: AssistantDeps) {
  const entries = new Map<string, Entry>();
  let seq = 0;
  const stamp = (): string => deps.now().toISOString();

  const entryOf = (dir: string): Entry => {
    const found = entries.get(dir);
    if (found) return found;
    const fresh: Entry = { state: emptyAssistantState(stamp()), phase: 'fresh', handles: [] };
    entries.set(dir, fresh);
    return fresh;
  };

  const message = (turn: number, role: AssistantRole, kind: AssistantKind, title: string | null, text: string): AssistantMessage => {
    seq += 1;
    return { id: `00000000-0000-4000-8000-${String(seq).padStart(12, '0')}`, ts: stamp(), turn, role, kind, title, text };
  };

  /** Replace the folder's state immutably and emit it. */
  const update = (dir: string, patch: Partial<AssistantState['session']>, added: AssistantMessage[] = [], phase?: Phase): AssistantState => {
    const entry = entryOf(dir);
    const state: AssistantState = {
      session: { ...entry.state.session, ...patch, updated_at: stamp() },
      messages: [...entry.state.messages, ...added],
    };
    entries.set(dir, { ...entry, state, phase: phase ?? entry.phase });
    deps.emitUpdate(clone(state));
    return clone(state);
  };

  const clearTimers = (dir: string): void => {
    const entry = entryOf(dir);
    entry.handles.forEach((h) => deps.timers.clearTimeout(h));
    entries.set(dir, { ...entry, handles: [] });
  };

  const later = (dir: string, ms: number, fn: () => void): void => {
    const entry = entryOf(dir);
    entries.set(dir, { ...entry, handles: [...entry.handles, deps.timers.setTimeout(fn, ms)] });
  };

  const emit = (kind: NodeEventKind, text: string): void => deps.emitEvent(ASSISTANT_RUN_ID, ASSISTANT_NODE_ID, kind, text);

  const finishTurn = (dir: string, turn: number, kind: AssistantKind, text: string): void => {
    clearTimers(dir);
    if (text.includes('FAKE_FAIL')) {
      emit('result', 'error · FAKE_FAIL (mock)');
      const error = 'the scheduler turn failed: FAKE_FAIL found in the message (mock)';
      update(dir, { status: 'failed', last_error: error }, [message(turn, 'app', 'error', null, error)]);
      return;
    }
    const { reply, next } = replyFor(deps.graphOf(dir), entryOf(dir).phase, kind);
    emit('result', `success · $0.02 · turn ${turn}`);
    const added = reply.map(([k, title, body]) => message(turn, 'scheduler', k, title, body));
    const status = added.some((m) => m.kind === 'question') ? 'waiting' : added.some((m) => m.kind === 'final') ? 'done' : 'idle';
    update(dir, { status, last_error: null }, added, next);
  };

  return {
    state: (dir: string): AssistantState => clone(entryOf(dir).state),

    send(dir: string, text: string): AssistantState {
      deps.graphOf(dir);
      const body = text.trim();
      if (body === '') throw appError('invalid_input', 'message is empty');
      const { session } = entryOf(dir).state;
      if (session.status === 'working') throw appError('assistant_busy', 'the scheduler is working; wait or cancel the turn');
      const kind: AssistantKind = session.status === 'waiting' ? 'answer' : 'instruction';
      const turn = session.turns + 1;
      const state = update(
        dir,
        {
          status: 'working',
          turns: turn,
          session_id: session.session_id ?? `mock-scheduler-${turn}`,
          model: MOCK_ASSISTANT_MODEL,
          workflow_hash: 'mock-hash',
          last_error: null,
        },
        [message(turn, 'user', kind, null, body)],
      );
      emit('user_message', body);
      const step = deps.replyMs / (SCRIPT.length + 1);
      SCRIPT.forEach(([k, line], i) => later(dir, step * (i + 1), () => emit(k, line)));
      later(dir, deps.replyMs, () => finishTurn(dir, turn, kind, body));
      return state;
    },

    cancel(dir: string): void {
      const { session } = entryOf(dir).state;
      if (session.status !== 'working') throw appError('not_found', 'no scheduler turn is running');
      clearTimers(dir);
      emit('system', `turn ${session.turns} cancelled`);
      update(dir, { status: 'cancelled' }, [message(session.turns, 'app', 'system', null, `Turn ${session.turns} cancelled`)]);
    },

    reset(dir: string): AssistantState {
      const { session } = entryOf(dir).state;
      if (session.status === 'working') throw appError('assistant_busy', 'cancel the running turn before resetting');
      return update(
        dir,
        { session_id: null, workflow_hash: null, status: 'idle', last_error: null },
        [message(0, 'app', 'system', null, MOCK_SESSION_RESET_TEXT)],
        'fresh',
      );
    },
  };
}

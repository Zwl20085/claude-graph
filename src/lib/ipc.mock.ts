/**
 * In-memory stand-in for the Tauri backend, used when the UI runs in a plain
 * browser. Graphs live in a Map keyed by folder; a run is simulated with
 * timers that emit the same `run:state` / `node:event` sequence the real
 * scheduler would (pending -> queued -> running -> succeeded|failed, failure
 * skips dependents, max_parallel respected). A node whose extra_context
 * contains FAKE_FAIL fails, like scripts/fake-claude.mjs.
 */
import example from '../../examples/hello-pipeline/graph.json';
import type {
  AppError,
  ClaudeCheck,
  Graph,
  NodeEvent,
  NodeEventKind,
  NodeResult,
  RunState,
  RunSummary,
  RunTarget,
} from '../types/contract';
import type { Api, Unlisten } from './api';
import { renderBrief } from './brief';
import { defaultTimers, type Timers } from './debounce';
import { emptyGraph } from './graphOps';
import { createMockRunner, type MockRun } from './mockRunner';
import { countStatuses } from './status';
import { validateGraph } from './validate';

export const MOCK_EXAMPLE_DIR = '/mock/hello-pipeline';

export interface MockOptions {
  timers?: Timers;
  /** Delay between simulated stream events of a node. */
  stepMs?: number;
  now?: () => Date;
  /** Seed the example graph (default true). */
  seed?: boolean;
  /** Folder answer for pick_folder (default: window.prompt). */
  pickFolder?: (title: string) => string | null;
}

const appError = (code: string, message: string): AppError => ({ code, message });
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

function promptFolder(title: string): string | null {
  const p = (globalThis as { prompt?: (msg: string, def?: string) => string | null }).prompt;
  if (typeof p !== 'function') return null;
  const answer = p(`${title}\n(mock mode: type any folder path)`, '/mock/new-graph');
  return answer === null || answer.trim() === '' ? null : answer.trim();
}

function summarise(run: MockRun): RunSummary {
  const costs = Object.values(run.state.nodes)
    .map((n) => n.cost_usd)
    .filter((c): c is number => c !== null);
  return {
    run_id: run.state.run_id,
    started_at: run.state.started_at,
    finished_at: run.state.finished_at,
    status: run.state.status,
    counts: countStatuses(run.state),
    total_cost_usd: costs.length === 0 ? null : costs.reduce((a, b) => a + b, 0),
  };
}

export function createMockIpc(options: MockOptions = {}): Api {
  const graphs = new Map<string, Graph>();
  let recent: string[] = [];
  const stateListeners = new Set<(s: RunState) => void>();
  const eventListeners = new Set<(e: NodeEvent) => void>();

  const touchRecent = (dir: string): void => {
    recent = [dir, ...recent.filter((d) => d !== dir)].slice(0, 20);
  };

  const runner = createMockRunner({
    timers: options.timers ?? defaultTimers,
    stepMs: options.stepMs ?? 600,
    now: options.now ?? (() => new Date()),
    emitState: (s) => stateListeners.forEach((h) => h(clone(s))),
    emitEvent: (dir, runId, nodeId, kind: NodeEventKind, text) => {
      void dir;
      const ev: NodeEvent = { run_id: runId, node_id: nodeId, kind, text, ts: (options.now ?? (() => new Date()))().toISOString() };
      eventListeners.forEach((h) => h(ev));
    },
  });

  if (options.seed !== false) {
    const g = validateGraph(example);
    if (!(g instanceof Error)) {
      graphs.set(MOCK_EXAMPLE_DIR, g);
      touchRecent(MOCK_EXAMPLE_DIR);
    }
  }

  const requireGraph = (dir: string): Graph => {
    const g = graphs.get(dir);
    if (!g) throw appError('not_found', `no graph.json in ${dir}`);
    return g;
  };

  const requireRun = (dir: string, runId: string): MockRun => {
    const run = runner.get(runId);
    if (!run || run.state.graph_dir !== dir) throw appError('not_found', `run ${runId} not found in ${dir}`);
    return run;
  };

  return {
    checkClaude: async (): Promise<ClaudeCheck> => ({ found: true, path: '/mock/claude', version: '2.1.0 (mock)' }),
    createGraph: async (dir, name) => {
      if (graphs.has(dir)) throw appError('io', `graph.json already exists in ${dir}`);
      const g = emptyGraph(name);
      graphs.set(dir, g);
      touchRecent(dir);
      return clone(g);
    },
    openGraph: async (dir) => {
      const g = requireGraph(dir);
      touchRecent(dir);
      return clone(g);
    },
    saveGraph: async (dir, graph) => {
      const g = validateGraph(clone(graph));
      if (g instanceof Error) throw appError('invalid_graph', g.message);
      graphs.set(dir, g);
      touchRecent(dir);
    },
    listRecent: async () => [...recent],
    pickFolder: async (title) => (options.pickFolder ?? promptFolder)(title),
    startRun: async (dir, target: RunTarget) => runner.start(dir, clone(requireGraph(dir)), target),
    cancelRun: async (runId) => {
      if (!runner.get(runId)) throw appError('not_found', `run ${runId} not found`);
      runner.cancel(runId);
    },
    cancelNode: async (runId, nodeId) => {
      if (!runner.get(runId)) throw appError('not_found', `run ${runId} not found`);
      runner.cancelNode(runId, nodeId);
    },
    getRun: async (dir, runId) => clone(requireRun(dir, runId).state),
    listRuns: async (dir) =>
      runner
        .list(dir)
        .map(summarise)
        .sort((a, b) => b.run_id.localeCompare(a.run_id)),
    readNodeResult: async (dir, runId, nodeId): Promise<NodeResult> => {
      const run = requireRun(dir, runId);
      if (!(nodeId in run.state.nodes)) throw appError('not_found', `node ${nodeId} not in run ${runId}`);
      return clone(run.results[nodeId] ?? { result_md: null, summary: null, events_tail: [] });
    },
    renderBrief: async (graph, nodeId) => {
      try {
        return renderBrief(graph, nodeId);
      } catch (err) {
        throw appError('not_found', err instanceof Error ? err.message : String(err));
      }
    },
    listMessages: async (dir, runId, nodeId) => clone(requireRun(dir, runId).messages[nodeId] ?? []),
    sendMessage: async (dir, runId, nodeId, text) => {
      requireRun(dir, runId);
      if (text.trim() === '') throw appError('invalid_input', 'message is empty');
      runner.sendMessage(runId, nodeId, text);
    },
    openPath: async () => undefined,
    onRunState: async (handler): Promise<Unlisten> => {
      stateListeners.add(handler);
      return () => stateListeners.delete(handler);
    },
    onNodeEvent: async (handler): Promise<Unlisten> => {
      eventListeners.add(handler);
      return () => eventListeners.delete(handler);
    },
  };
}

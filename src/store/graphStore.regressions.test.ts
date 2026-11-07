/**
 * Regression tests for the review findings, driving the store with a
 * controllable fake API (deferred promises decide the interleaving).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NodeEvent, NodeRunState, NodeStatus, RunState, RunSummary } from '../types/contract';
import type { Api } from '../lib/api';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { emptyCounts } from '../lib/status';
import { createGraphStore } from './graphStore';
import { preferFetched, progressOf } from './runHelpers';

const DIR = MOCK_EXAMPLE_DIR;

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (v: T) => void;
  reject: (e: unknown) => void;
}
function deferred<T>(): Deferred<T> {
  let resolve: (v: T) => void = () => undefined;
  let reject: (e: unknown) => void = () => undefined;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const nodeState = (status: NodeStatus): NodeRunState => ({
  status, started_at: null, finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: null,
});
function run(run_id: string, statuses: Record<string, NodeStatus>, graph_dir = DIR): RunState {
  const done = Object.values(statuses).every((s) => s === 'succeeded');
  return {
    run_id,
    graph_dir,
    started_at: '2026-11-07T10:00:00Z',
    finished_at: done ? '2026-11-07T10:01:00Z' : null,
    status: done ? 'succeeded' : 'running',
    nodes: Object.fromEntries(Object.entries(statuses).map(([id, s]) => [id, nodeState(s)])),
  };
}
const summary = (rs: RunState): RunSummary => ({
  run_id: rs.run_id, started_at: rs.started_at, finished_at: rs.finished_at, status: rs.status, counts: emptyCounts(), total_cost_usd: null,
});
const tick = () => vi.advanceTimersByTimeAsync(0);

function setup(overrides: Partial<Api> = {}, trustAll = true) {
  const base = createMockIpc({ stepMs: 50, trustAll });
  const api: Api = { ...base, ...overrides };
  const store = createGraphStore(api);
  void base.onRunState((rs) => store.getState().applyRunState(rs));
  void base.onNodeEvent((ev) => store.getState().appendEvent(ev));
  return { base, api, store, s: () => store.getState() };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('H1: get_run reply vs live run:state', () => {
  it('a late get_run reply does not overwrite a newer live state', async () => {
    const reply = deferred<RunState>();
    const getRun = vi.fn(() => reply.promise);
    const { s } = setup({ startRun: async () => 'R1', getRun, listRuns: async () => [] });
    await s().openGraph(DIR);
    const started = s().startRun('all');
    await tick();
    expect(getRun).toHaveBeenCalledWith(DIR, 'R1');
    const live = run('R1', { survey: 'succeeded', design: 'running', report: 'pending' });
    s().applyRunState(live);
    reply.resolve(run('R1', { survey: 'running', design: 'pending', report: 'pending' }));
    expect(await started).toBe(true);
    expect(s().runState).toBe(live);
  });

  it('a get_run reply that is further along replaces the shown state', async () => {
    const reply = deferred<RunState>();
    const { s } = setup({ startRun: async () => 'R1', getRun: () => reply.promise, listRuns: async () => [] });
    await s().openGraph(DIR);
    const started = s().startRun('all');
    await tick();
    // A state of another spelling of the folder is buffered, then dropped for a newer reply.
    s().applyRunState(run('R1', { survey: 'running', design: 'pending', report: 'pending' }));
    const further = run('R1', { survey: 'succeeded', design: 'succeeded', report: 'running' });
    reply.resolve(further);
    await started;
    expect(s().runState).toEqual(further);
  });

  it('run:state and node:event of the new run that arrive before start_run resolves are buffered, then applied', async () => {
    const start = deferred<string>();
    const getRun = vi.fn(async () => run('R1', {}));
    const { s } = setup({ startRun: () => start.promise, getRun, listRuns: async () => [] });
    await s().openGraph(DIR);
    const started = s().startRun('all');
    await tick();
    const canonical = run('R1', { survey: 'running', design: 'pending', report: 'pending' }, 'C:/long/form/hello');
    s().applyRunState(canonical);
    const ev: NodeEvent = { run_id: 'R1', node_id: 'survey', kind: 'text', text: 'hi', ts: 't' };
    s().appendEvent(ev);
    s().appendEvent({ ...ev, run_id: 'R0', text: 'foreign' });
    expect(s().runState).toBeNull();
    start.resolve('R1');
    expect(await started).toBe(true);
    expect(s().runState).toEqual(canonical);
    expect(getRun).not.toHaveBeenCalled();
    expect(s().events.survey?.map((l) => l.text)).toEqual(['hi']);
    // After the start, events of unknown runs are dropped again.
    s().appendEvent({ ...ev, run_id: 'R9' });
    s().applyRunState(run('R9', {}, '/elsewhere'));
    expect(s().runState?.run_id).toBe('R1');
  });

  it('preferFetched / progressOf', () => {
    const a = run('R', { x: 'running' });
    const b = run('R', { x: 'succeeded' });
    expect(progressOf(b)).toBe(2);
    expect(preferFetched(null, a)).toBe(a);
    expect(preferFetched(a, b)).toBe(b);
    expect(preferFetched(b, a)).toBe(b);
    expect(preferFetched(run('Q', {}), a)).toBe(a);
  });

  it('malformed run:state payloads are ignored', async () => {
    const { s } = setup({ listRuns: async () => [] });
    await s().openGraph(DIR);
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    s().applyRunState({ run_id: 'x', graph_dir: DIR });
    expect(s().runState).toBeNull();
    warn.mockRestore();
  });
});

describe('M3: switching graphs', () => {
  it('an edit made while closeGraph saves is saved too before the switch', async () => {
    const { base, s } = setup();
    const first = deferred<void>();
    const saveGraph = vi.fn(base.saveGraph);
    saveGraph.mockImplementationOnce(async (dir, graph) => {
      await first.promise;
      return base.saveGraph(dir, graph);
    });
    const store = createGraphStore({ ...base, saveGraph });
    const t = () => store.getState();
    await t().openGraph(DIR);
    t().setName('one');
    const switching = t().createGraph('/mock/next', 'next');
    await tick();
    expect(saveGraph).toHaveBeenCalledTimes(1);
    t().setName('two');
    first.resolve();
    expect(await switching).toBe(true);
    expect(saveGraph).toHaveBeenCalledTimes(2);
    expect((await base.openGraph(DIR)).name).toBe('two');
    expect(t().dir).toBe('/mock/next');
    void s;
  });

  it('a graph that fails to load leaves the open graph untouched (loaded first, then closed)', async () => {
    const { s, base } = setup();
    const saveGraph = vi.fn(base.saveGraph);
    const store = createGraphStore({ ...base, saveGraph });
    await store.getState().openGraph(DIR);
    store.getState().setName('kept');
    expect(await store.getState().openGraph('/mock/missing')).toBe(false);
    expect(store.getState().dir).toBe(DIR);
    expect(store.getState().dirty).toBe(true);
    expect(saveGraph).not.toHaveBeenCalled();
    void s;
  });

  it('closeGraph gives up when every save round finds new edits', async () => {
    const { base } = setup();
    let store = createGraphStore(base);
    const saveGraph = vi.fn(async () => {
      store.getState().setName(`n${saveGraph.mock.calls.length}`);
    });
    store = createGraphStore({ ...base, saveGraph });
    await store.getState().openGraph(DIR);
    store.getState().setName('x');
    expect(await store.getState().closeGraph()).toBe(false);
    expect(store.getState().dir).toBe(DIR);
  });
});

describe('M4: latest run lookup vs a new run', () => {
  it('a run started during loadLatestRun is not replaced by the old run', async () => {
    const old = run('R0', { survey: 'succeeded', design: 'succeeded', report: 'succeeded' });
    const oldReply = deferred<RunState>();
    const fresh = run('R1', { survey: 'running', design: 'pending', report: 'pending' });
    const { s } = setup({
      listRuns: async () => [summary(old)],
      getRun: (_dir, id) => (id === 'R0' ? oldReply.promise : Promise.resolve(fresh)),
      startRun: async () => 'R1',
    });
    await s().openGraph(DIR);
    await tick();
    expect(await s().startRun('all')).toBe(true);
    expect(s().runState?.run_id).toBe('R1');
    oldReply.resolve(old);
    await tick();
    expect(s().runState?.run_id).toBe('R1');
  });

  it('a lookup that resolves while a start is pending is dropped', async () => {
    const old = run('R0', { survey: 'succeeded' });
    const oldReply = deferred<RunState>();
    const start = deferred<string>();
    const { s } = setup({
      listRuns: async () => [summary(old)],
      getRun: (_dir, id) => (id === 'R0' ? oldReply.promise : Promise.resolve(run('R1', { survey: 'running' }))),
      startRun: () => start.promise,
    });
    await s().openGraph(DIR);
    const started = s().startRun('all');
    await tick();
    oldReply.resolve(old);
    await tick();
    expect(s().runState).toBeNull();
    start.resolve('R1');
    await started;
    expect(s().runState?.run_id).toBe('R1');
  });
});

describe('M6: double start', () => {
  it('a second startRun while the first is pending is refused synchronously', async () => {
    const start = deferred<string>();
    const startRun = vi.fn(() => start.promise);
    const { s } = setup({ startRun, listRuns: async () => [], getRun: async () => run('R1', {}) });
    await s().openGraph(DIR);
    const first = s().startRun('all');
    const second = s().startRun('all');
    expect(s().startingRun).toBe(true);
    expect(await second).toBe(false);
    start.resolve('R1');
    expect(await first).toBe(true);
    expect(startRun).toHaveBeenCalledTimes(1);
    expect(s().startingRun).toBe(false);
  });

  it('startingRun resets after a failed start', async () => {
    const { s } = setup({ startRun: async () => Promise.reject({ code: 'spawn_failed', message: 'boom' }), listRuns: async () => [] });
    await s().openGraph(DIR);
    expect(await s().startRun('all')).toBe(false);
    expect(s().startingRun).toBe(false);
    expect(s().toasts[s().toasts.length - 1]?.text).toMatch(/boom/);
  });
});

describe('L3: fresh node ids', () => {
  it('only the nodes of the target set lose their editable id', async () => {
    const { s } = setup();
    await s().openGraph(DIR);
    const a = s().addNodeAt({ x: 0, y: 0 }) as string;
    s().updateNode(a, { prompt: 'p' });
    const b = s().addNodeAt({ x: 0, y: 0 }) as string;
    s().updateNode(b, { prompt: 'p' });
    expect(s().connect('report', a)).toBe(true);
    expect(await s().startRun({ from: 'report' })).toBe(true);
    expect(s().freshNodeIds).toEqual([b]);
    await vi.runAllTimersAsync();
  });
});

describe('C2: trust dialog', () => {
  it('asks before the first run, cancel refuses, accept trusts and runs, then no more asking', async () => {
    const { s, api } = setup({}, false);
    const startRun = vi.spyOn(api, 'startRun');
    await s().openGraph(DIR);
    const refused = s().startRun('all');
    await tick();
    expect(s().trustRequest?.items.map((i) => i.kind)).toEqual(['bash_tool']);
    s().resolveTrust(false);
    expect(await refused).toBe(false);
    expect(startRun).not.toHaveBeenCalled();

    const accepted = s().startRun('all');
    await tick();
    s().resolveTrust(true);
    expect(await accepted).toBe(true);
    expect(startRun).toHaveBeenCalledTimes(1);
    await vi.runAllTimersAsync();
    expect(s().runState?.status).toBe('succeeded');
    const again = s().startRun('all');
    await tick();
    expect(s().trustRequest).toBeNull();
    expect(await again).toBe(true);
    await vi.runAllTimersAsync();
  });

  it('changing an elevated field asks again; closing the graph cancels the dialog', async () => {
    const { s, base } = setup({}, false);
    await base.trustGraph(DIR);
    await s().openGraph(DIR);
    s().updateNode('report', { permission_mode: 'bypassPermissions' });
    const pending = s().startRun('all');
    await vi.advanceTimersByTimeAsync(10);
    expect(s().trustRequest?.items.some((i) => i.node_id === 'report' && i.kind === 'bypass_permissions')).toBe(true);
    expect(await s().closeGraph()).toBe(true);
    expect(await pending).toBe(false);
    expect(s().trustRequest).toBeNull();
  });

  it('untrusted_graph from start_run asks once more and retries', async () => {
    let calls = 0;
    let checks = 0;
    const { base } = setup({}, false);
    const item = { node_id: 'design', kind: 'bash_tool' as const, detail: 'Bash' };
    const store = createGraphStore({
      ...base,
      // First check says trusted (stale), the re-check after the refusal says not.
      graphTrustStatus: async () => {
        checks += 1;
        return checks === 1 ? { trusted: true, items: [] } : { trusted: false, items: [item] };
      },
      startRun: async (dir, target) => {
        calls += 1;
        if (calls === 1) throw { code: 'untrusted_graph', message: 'not trusted' };
        await base.trustGraph(dir);
        return base.startRun(dir, target);
      },
    });
    await store.getState().openGraph(DIR);
    const p = store.getState().startRun('all');
    await tick();
    expect(store.getState().trustRequest?.items).toEqual([item]);
    store.getState().resolveTrust(true);
    expect(await p).toBe(true);
    expect(calls).toBe(2);
    await vi.runAllTimersAsync();
  });

  it('a failing trust check toasts and refuses', async () => {
    const { s } = setup({ graphTrustStatus: async () => Promise.reject({ code: 'io', message: 'nope' }) }, false);
    await s().openGraph(DIR);
    expect(await s().ensureTrusted(DIR)).toBe(false);
    expect(s().toasts[s().toasts.length - 1]?.text).toMatch(/trusted: nope/);
  });

  it('a failing trust_graph toasts and refuses', async () => {
    const { s } = setup({ trustGraph: async () => Promise.reject({ code: 'io', message: 'ro' }) }, false);
    await s().openGraph(DIR);
    const p = s().ensureTrusted(DIR);
    await tick();
    s().resolveTrust(true);
    expect(await p).toBe(false);
    expect(s().toasts[s().toasts.length - 1]?.text).toMatch(/ro/);
  });
});

describe('C1: app settings and claude_bin in graph.json', () => {
  it('loads, saves, validates the claude path and re-checks claude', async () => {
    const { s } = setup();
    await s().init();
    expect(s().appSettings).toEqual({ claude_bin: null });
    expect(await s().saveAppSettings({ claude_bin: 'relative/claude' })).toMatch(/absolute/);
    expect(await s().saveAppSettings({ claude_bin: 'C:/tools/claude.cmd' })).toBeNull();
    expect(s().appSettings?.claude_bin).toBe('C:/tools/claude.cmd');
    expect(s().claude).toMatchObject({ kind: 'ok', check: { path: 'C:/tools/claude.cmd' } });
  });

  it('an opened graph that names claude_bin shows the "ignored" info', async () => {
    const { s, base } = setup();
    const g = await base.openGraph(DIR);
    await base.saveGraph(DIR, { ...g, settings: { ...g.settings, claude_bin: '/x/claude' } });
    await s().openGraph(DIR);
    expect(s().toasts.some((t) => t.kind === 'info' && t.text.includes('per-machine setting'))).toBe(true);
  });

  it('a failing get_app_settings is logged, not fatal', async () => {
    const { s } = setup({ getAppSettings: async () => Promise.reject({ code: 'io', message: 'x' }) });
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    await s().loadAppSettings();
    expect(s().appSettings).toBeNull();
    warn.mockRestore();
  });
});

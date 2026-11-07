import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppError, Graph, NodeEvent, NodeStatus, RunState } from '../types/contract';
import { createMockIpc, MOCK_EXAMPLE_DIR } from './ipc.mock';
import { runIdFor, targetSet } from './mockRunner';
import { updateNode } from './graphOps';

const STEP = 100;
const last = <T>(a: readonly T[]): T | undefined => a[a.length - 1];
const DIR = MOCK_EXAMPLE_DIR;

function setup() {
  const api = createMockIpc({ stepMs: STEP, now: () => new Date(Date.now()), pickFolder: () => '/mock/picked' });
  const states: RunState[] = [];
  const events: NodeEvent[] = [];
  void api.onRunState((s) => states.push(s));
  void api.onNodeEvent((e) => events.push(e));
  /** Distinct status snapshots of one node across all emitted states. */
  const trail = (id: string): string[] =>
    states
      .map((s) => s.nodes[id]?.status)
      .filter((s): s is NonNullable<typeof s> => s !== undefined)
      .filter((s, i, a) => i === 0 || a[i - 1] !== s);
  return { api, states, events, trail };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-07T10:15:00Z'));
});
afterEach(() => vi.useRealTimers());

describe('mock run sequencing', () => {
  it('runs the example chain in dependency order and succeeds', async () => {
    const { api, states, events, trail } = setup();
    const runId = await api.startRun(DIR, 'all');
    expect(runId).toBe('20261107T101500Z');
    await vi.runAllTimersAsync();

    expect(trail('survey')).toEqual(['pending', 'queued', 'running', 'succeeded']);
    expect(trail('design')).toEqual(['pending', 'queued', 'running', 'succeeded']);
    const final = last(states);
    expect(final?.status).toBe('succeeded');
    expect(final?.finished_at).not.toBeNull();

    const firstDesign = events.findIndex((e) => e.node_id === 'design');
    const surveyResult = events.findIndex((e) => e.node_id === 'survey' && e.kind === 'result');
    expect(surveyResult).toBeLessThan(firstDesign);
    expect(events.filter((e) => e.node_id === 'survey').map((e) => e.kind)).toEqual([
      'system', 'text', 'tool_use', 'tool_result', 'text', 'result',
    ]);

    const runs = await api.listRuns(DIR);
    expect(runs[0]).toMatchObject({ run_id: runId, status: 'succeeded', counts: { succeeded: 3 } });
    expect(runs[0]?.total_cost_usd).toBeGreaterThan(0);
    const res = await api.readNodeResult(DIR, runId, 'design');
    expect(res.result_md).toContain('## Status: DONE');
    expect(res.summary?.status).toBe('succeeded');
  });

  it('FAKE_FAIL fails a node and skips its dependents; rerun failed targets them', async () => {
    const { api, states, trail } = setup();
    const g = await api.openGraph(DIR);
    await api.saveGraph(DIR, updateNode(g, 'design', { extra_context: 'FAKE_FAIL' }));
    const runId = await api.startRun(DIR, 'all');
    await vi.runAllTimersAsync();
    expect(last(trail('design'))).toBe('failed');
    expect(trail('report')).toEqual(['pending', 'skipped']);
    expect(last(states)?.status).toBe('failed');
    expect(last(states)?.nodes.report?.reason).toMatch(/upstream design failed/);

    vi.setSystemTime(new Date('2026-11-07T10:20:00Z'));
    const rerun = await api.startRun(DIR, { failed_of: runId });
    expect(Object.keys((await api.getRun(DIR, rerun)).nodes)).toEqual(['design', 'report']);
  });

  it('respects max_parallel', async () => {
    const { api, states } = setup();
    const g = await api.createGraph('/mock/par', 'par');
    const three: Graph = {
      ...g,
      settings: { ...g.settings, max_parallel: 2 },
      nodes: ['a', 'b', 'c'].map((id) => ({ ...(await_node(id)) })),
    };
    await api.saveGraph('/mock/par', three);
    await api.startRun('/mock/par', 'all');
    await vi.runAllTimersAsync();
    const maxRunning = Math.max(...states.map((s) => Object.values(s.nodes).filter((n) => n.status === 'running').length));
    expect(maxRunning).toBe(2);
    expect(last(states)?.status).toBe('succeeded');
  });

  it('cancel stops a run; a second run while active is refused', async () => {
    const { api, states } = setup();
    const runId = await api.startRun(DIR, 'all');
    await expect(api.startRun(DIR, 'all')).rejects.toMatchObject({ code: 'run_active' } satisfies Partial<AppError>);
    await vi.advanceTimersByTimeAsync(STEP * 2);
    await api.cancelRun(runId);
    await vi.runAllTimersAsync();
    const final = last(states);
    expect(final?.status).toBe('cancelled');
    expect(Object.values(final?.nodes ?? {}).every((n: { status: string }) => n.status === 'cancelled')).toBe(true);
  });

  it('cancel_node cancels one node and skips its dependents', async () => {
    const { api, states } = setup();
    const runId = await api.startRun(DIR, { from: 'design' });
    await vi.advanceTimersByTimeAsync(STEP);
    await api.cancelNode(runId, 'design');
    await vi.runAllTimersAsync();
    expect(last(states)?.nodes).toMatchObject({ design: { status: 'cancelled' }, report: { status: 'skipped' } });
    expect(last(states)?.status).toBe('cancelled');
  });
});

// helper outside the test to keep the object literal simple
function await_node(id: string) {
  return {
    id, title: id, position: { x: 0, y: 0 }, prompt: 'p', goal: '', acceptance: '', scope_in: '', scope_out: '', already_known: '',
    deliverables: [], do_nots: [], model: 'inherit' as const, resources: [], permission_mode: 'inherit' as const,
    allowed_tools: null, max_turns: null, working_dir: null, continue_session_of: null, extra_context: '',
  };
}

describe('mock graph commands', () => {
  it('open/save/create/recent/pick/check/render', async () => {
    const { api } = setup();
    expect(await api.listRecent()).toEqual([DIR]);
    await expect(api.openGraph('/nope')).rejects.toMatchObject({ code: 'not_found' });
    await api.createGraph('/mock/new', 'new');
    await expect(api.createGraph('/mock/new', 'new')).rejects.toMatchObject({ code: 'io' });
    expect(await api.listRecent()).toEqual(['/mock/new', DIR]);
    const g = await api.openGraph(DIR);
    await expect(api.saveGraph(DIR, { ...g, edges: [{ id: 'x', from: 'report', to: 'survey' }, ...g.edges] })).rejects.toMatchObject({
      code: 'invalid_graph',
    });
    expect(await api.pickFolder('t')).toBe('/mock/picked');
    expect((await api.checkClaude()).found).toBe(true);
    expect(await api.renderBrief(g, 'survey')).toContain('# Node: Literature survey');
    await expect(api.renderBrief(g, 'ghost')).rejects.toMatchObject({ code: 'not_found' });
    await expect(api.getRun(DIR, 'none')).rejects.toMatchObject({ code: 'not_found' });
    await expect(api.cancelRun('none')).rejects.toMatchObject({ code: 'not_found' });
    await expect(api.startRun(DIR, { from: 'ghost' })).rejects.toMatchObject({ code: 'not_found' });
    await expect(api.openPath('/x')).resolves.toBeUndefined();
  });

  it('unlisten stops delivery; mutating a returned graph does not touch the store', async () => {
    const { api } = setup();
    const seen: RunState[] = [];
    const off = await api.onRunState((s) => seen.push(s));
    off();
    await api.startRun(DIR, 'all');
    await vi.runAllTimersAsync();
    expect(seen).toEqual([]);
    const g = await api.openGraph(DIR);
    (g.nodes as unknown[]).length = 0;
    expect((await api.openGraph(DIR)).nodes).toHaveLength(3);
  });

  it('helpers: run id format and target sets', async () => {
    expect(runIdFor(new Date('2026-11-07T10:15:00.123Z'))).toBe('20261107T101500Z');
    const { api } = setup();
    const g = await api.openGraph(DIR);
    expect(targetSet(g, { from: 'design' }, () => undefined)).toEqual(['design', 'report']);
    expect(() => targetSet(g, { failed_of: 'x' }, () => undefined)).toThrow();
    // Rerun failed = failed + skipped + cancelled of that run, plus dependents (backend rule).
    const node = (status: NodeStatus) => ({ status, started_at: null, finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: null });
    const prev = (statuses: Record<string, NodeStatus>): RunState => ({
      run_id: 'R', graph_dir: DIR, started_at: '', finished_at: '', status: 'cancelled',
      nodes: Object.fromEntries(Object.entries(statuses).map(([id, st]) => [id, node(st)])),
    });
    expect(targetSet(g, { failed_of: 'R' }, () => prev({ survey: 'succeeded', design: 'cancelled', report: 'skipped' }))).toEqual(['design', 'report']);
    expect(() => targetSet(g, { failed_of: 'R' }, () => prev({ survey: 'succeeded' }))).toThrow(
      expect.objectContaining({ code: 'not_found' }),
    );
  });
});

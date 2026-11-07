import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Graph, NodeEvent, RunState } from '../types/contract';
import type { Api } from '../lib/api';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { AUTOSAVE_MS, createGraphStore, emptyTargets } from './graphStore';
import { acceptsRunState, addEvent, appendCapped, newestFirst, samePath } from './runHelpers';

const DIR = MOCK_EXAMPLE_DIR;

function setup(overrides: Partial<Api> = {}) {
  const base = createMockIpc({ stepMs: 50 });
  const saveSpy = vi.fn(base.saveGraph);
  const api: Api = { ...base, saveGraph: saveSpy, ...overrides };
  const store = createGraphStore(api);
  void api.onRunState((s) => store.getState().applyRunState(s));
  void api.onNodeEvent((e) => store.getState().appendEvent(e));
  return { api, base, store, saveSpy, s: () => store.getState() };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('graph store: open and autosave', () => {
  it('opens a graph clean, edits mark dirty, autosave fires 800 ms after the last edit', async () => {
    const { s, saveSpy, base } = setup();
    expect(await s().openGraph(DIR)).toBe(true);
    expect(s()).toMatchObject({ dir: DIR, dirty: false, selectedNodeId: null });
    const before = s().graph as Graph;

    s().updateNode('survey', { goal: 'g1' });
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS - 100);
    s().updateNode('survey', { goal: 'g2' });
    expect(before.nodes[0]?.goal).not.toBe('g2');
    expect(s().dirty).toBe(true);
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS - 1);
    expect(saveSpy).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(saveSpy).toHaveBeenCalledTimes(1);
    expect(s().dirty).toBe(false);
    expect((await base.openGraph(DIR)).nodes[0]?.goal).toBe('g2');
  });

  it('no-op edits do not mark dirty; save() writes immediately', async () => {
    const { s, saveSpy } = setup();
    await s().openGraph(DIR);
    s().moveNodes({ survey: { x: 80, y: 160 } });
    expect(s().dirty).toBe(false);
    s().moveNodes({ survey: { x: 96, y: 160 } });
    expect(await s().save()).toBe(true);
    expect(saveSpy).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS * 2);
    expect(saveSpy).toHaveBeenCalledTimes(1);
  });

  it('an edit during a slow save keeps the graph dirty and saves again', async () => {
    let release: () => void = () => undefined;
    const { s, saveSpy } = setup();
    saveSpy.mockImplementationOnce(() => new Promise<void>((r) => (release = r)));
    await s().openGraph(DIR);
    s().setName('one');
    const first = s().save();
    await vi.advanceTimersByTimeAsync(0);
    expect(s().saving).toBe(true);
    s().setName('two');
    release();
    await first;
    expect(s().dirty).toBe(true);
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS);
    expect(saveSpy).toHaveBeenCalledTimes(2);
    expect(saveSpy.mock.calls[1]?.[1].name).toBe('two');
    expect(s().dirty).toBe(false);
  });

  it('a failed save keeps dirty, toasts once, and blocks switching graphs', async () => {
    const { s, saveSpy } = setup();
    saveSpy.mockRejectedValue({ code: 'io', message: 'disk full' });
    await s().openGraph(DIR);
    s().setName('x');
    expect(await s().save()).toBe(false);
    expect(await s().save()).toBe(false);
    expect(s().dirty).toBe(true);
    expect(s().toasts.filter((t) => t.text.includes('disk full'))).toHaveLength(1);
    expect(await s().openGraph('/mock/other')).toBe(false);
    expect(s().dir).toBe(DIR);
  });

  it('createGraph, open errors, settings, recent', async () => {
    const { s } = setup();
    await s().init();
    expect(s().recent).toEqual([DIR]);
    expect(s().claude.kind).toBe('ok');
    expect(await s().createGraph('/mock/n', 'n')).toBe(true);
    expect(s().graph?.name).toBe('n');
    s().updateSettings({ max_parallel: 4, claude_bin: '/x/claude' });
    expect(s().graph?.settings.max_parallel).toBe(4);
    expect(await s().openGraph('/missing')).toBe(false);
    expect(s().toasts.some((t) => t.text.includes('/missing'))).toBe(true);
    s().dismissToast(s().toasts[0]?.id ?? -1);
  });
});

describe('graph store: editing', () => {
  it('adds, renames (fresh only), connects, refuses cycles, removes', async () => {
    const { s } = setup();
    await s().openGraph(DIR);
    const id = s().addNodeAt({ x: 0, y: 0 });
    expect(id).toBe('new-node');
    expect(s().selectedNodeId).toBe('new-node');
    expect(s().renameNodeId('new-node', 'checker')).toBeNull();
    expect(s().selectedNodeId).toBe('checker');
    expect(s().renameNodeId('survey', 'x')).toMatch(/cannot change/);

    expect(s().connect('report', 'checker')).toBe(true);
    expect(s().connect('checker', 'survey')).toBe(false);
    expect(s().toasts[s().toasts.length - 1]?.text).toMatch(/cycle/);
    expect(s().connect('report', 'checker')).toBe(false);

    s().removeElements(['checker'], []);
    expect(s().graph?.nodes.map((n) => n.id)).toEqual(['survey', 'design', 'report']);
    expect(s().graph?.edges.map((e) => e.id)).toEqual(['e1', 'e2']);
    expect(s().selectedNodeId).toBeNull();
    s().removeElements([], ['e2']);
    expect(s().graph?.edges.map((e) => e.id)).toEqual(['e1']);
  });
});

describe('graph store: runs', () => {
  it('saves pending edits, runs, tracks run:state and node events, then reloads the latest run on open', async () => {
    const { s, saveSpy } = setup();
    await s().openGraph(DIR);
    s().updateNode('report', { prompt: 'write it' });
    expect(await s().startRun('all')).toBe(true);
    expect(saveSpy).toHaveBeenCalledTimes(1);
    await vi.runAllTimersAsync();
    expect(s().runState?.status).toBe('succeeded');
    expect(s().events.survey?.map((l) => l.kind)).toContain('result');

    s().clearEvents('survey');
    expect(s().events.survey).toEqual([]);
    await s().openGraph(DIR);
    await vi.runAllTimersAsync();
    expect(s().runState?.status).toBe('succeeded');
  });

  it('refuses to run empty nodes with a toast', async () => {
    const { s } = setup();
    await s().openGraph(DIR);
    s().addNodeAt({ x: 0, y: 0 });
    expect(await s().startRun('all')).toBe(false);
    expect(s().toasts[s().toasts.length - 1]?.text).toMatch(/new-node/);
    expect(await s().startRun({ from: 'report' })).toBe(true);
    await s().cancelRun();
    await vi.runAllTimersAsync();
    expect(s().runState?.status).toBe('cancelled');
  });

  it('ignores run:state and node:event of other graphs and runs', async () => {
    const { s } = setup();
    await s().openGraph(DIR);
    const foreign: RunState = { run_id: 'zz', graph_dir: '/other', started_at: 'x', finished_at: null, status: 'running', nodes: {} };
    s().applyRunState(foreign);
    expect(s().runState).toBeNull();
    const ev: NodeEvent = { run_id: 'zz', node_id: 'survey', kind: 'text', text: 'x', ts: 't' };
    s().appendEvent(ev);
    expect(s().events).toEqual({});
  });
});

describe('run helpers', () => {
  it('appendCapped drops the oldest beyond the cap', () => {
    expect(appendCapped([1, 2, 3], 4, 3)).toEqual([2, 3, 4]);
    const ev: NodeEvent = { run_id: 'r', node_id: 'a', kind: 'text', text: 't', ts: 'x' };
    expect(addEvent({}, ev, 1).a).toEqual([{ seq: 1, kind: 'text', text: 't', ts: 'x' }]);
  });

  it('samePath / acceptsRunState / newestFirst', () => {
    expect(samePath('C:\\G\\x\\', 'c:/g/x')).toBe(true);
    expect(samePath('/a/B', '/a/b')).toBe(false);
    expect(samePath(null, '/a')).toBe(false);
    const rs = { run_id: 'r', graph_dir: '/g' } as RunState;
    expect(acceptsRunState(rs, '/g/', null, false)).toBe(true);
    expect(acceptsRunState(rs, '/h', null, true)).toBe(true);
    expect(acceptsRunState(rs, '/h', null, false)).toBe(false);
    const mk = (run_id: string, started_at: string) => ({ run_id, started_at }) as Parameters<typeof newestFirst>[0][number];
    expect(newestFirst([mk('a', '1'), mk('b', '2')]).map((r) => r.run_id)).toEqual(['b', 'a']);
  });

  it('emptyTargets falls back to the whole graph for unknown runs', async () => {
    const { base } = setup();
    const g = await base.openGraph(DIR);
    const withEmpty: Graph = { ...g, nodes: g.nodes.map((n) => (n.id === 'survey' ? { ...n, goal: '' } : n)) };
    expect(emptyTargets(withEmpty, { failed_of: 'nope' }, null)).toEqual(['survey']);
    expect(emptyTargets(withEmpty, { from: 'design' }, null)).toEqual([]);
  });
});

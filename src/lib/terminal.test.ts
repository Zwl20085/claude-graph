/** Terminal mode, pure parts and the mock backend: routing, session grouping, fake ptys, cross-tab bridge. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Graph, NodeRunState, PtyData, RunState } from '../types/contract';
import { runModeOf, runStateMode, sessionsOf } from '../types/contract';
import { parseHashRoute } from '../hooks/useHashRoute';
import { addNode, connect, emptyGraph, updateNode, updateSettings } from './graphOps';
import { createMockIpc, MOCK_EXAMPLE_DIR } from './ipc.mock';
import type { MockChannel } from './mockTerminal';
import { terminalMessage } from './mockTerminal';
import { deriveSession, groupSessions, isSessionAlive, sessionOfNode } from './sessions';
import { validateGraph } from './validate';

const last = <T>(a: readonly T[]): T | undefined => a[a.length - 1];
const linked = (g: Graph, from: string, to: string): Graph => {
  const r = connect(g, from, to);
  if (!r.ok) throw new Error(r.error);
  return r.graph;
};

describe('parseHashRoute', () => {
  it('routes #/terminal/<run>/<sid> to the terminal window', () => {
    expect(parseHashRoute('#/terminal/20261107T101500Z/s2')).toEqual({ kind: 'terminal', runId: '20261107T101500Z', sid: 's2', dir: null });
    expect(parseHashRoute('#/terminal/r-1/s1?dir=C%3A%2Fg%20x')).toEqual({ kind: 'terminal', runId: 'r-1', sid: 's1', dir: 'C:/g x' });
  });
  it('anything else is the main window', () => {
    for (const h of ['', '#', '#/', '#/terminal/run', '#/terminal//s1', '#/terminal/a b/s1', '#/terminal/r/s/extra', '#/terminal/%E0%A4%A/s1']) {
      expect(parseHashRoute(h)).toEqual({ kind: 'main' });
    }
  });
});

function chainAndSolo(): Graph {
  let g = emptyGraph('g');
  const a = addNode(g, 'Alpha', { x: 300, y: 100 });
  const b = addNode(a.graph, 'Beta', { x: 500, y: 100 });
  const c = addNode(b.graph, 'Gamma', { x: 0, y: 50 });
  const d = addNode(c.graph, 'Delta', { x: 0, y: 300 });
  g = linked(d.graph, a.id, b.id);
  g = linked(g, d.id, b.id);
  return g;
}

describe('run mode defaults', () => {
  it('new graphs are terminal; a graph.json without run_mode reads as terminal and stays without it', () => {
    expect(emptyGraph('x').settings.run_mode).toBe('terminal');
    const g = validateGraph({ version: 1, name: 'old', settings: { max_parallel: 1 } });
    if (g instanceof Error) throw g;
    expect('run_mode' in g.settings).toBe(false);
    expect(runModeOf(g.settings)).toBe('terminal');
    expect(runModeOf({ run_mode: 'headless' })).toBe('headless');
    const h = validateGraph({ version: 1, name: 'h', settings: { run_mode: 'headless' } });
    expect(h instanceof Error ? h : h.settings.run_mode).toBe('headless');
    expect(validateGraph({ version: 1, name: 'bad', settings: { run_mode: 'tty' } })).toBeInstanceOf(Error);
  });
  it('RunState mode and sessions have safe defaults', () => {
    expect(runStateMode({})).toBe('headless');
    expect(runStateMode({ mode: 'terminal' })).toBe('terminal');
    expect(sessionsOf({})).toEqual([]);
    expect(sessionsOf({ sessions: [{ sid: 's1' }, { sid: 's2', title: 't', node_ids: ['a'], status: 'done' }] })).toEqual([
      { sid: 's2', title: 't', node_ids: ['a'], status: 'done', pid: null, current_node: null },
    ]);
  });
});

describe('groupSessions', () => {
  it('one session per weakly connected component, numbered by top-left node, nodes in send order', () => {
    const g = chainAndSolo();
    const s = groupSessions(g, g.nodes.map((n) => n.id));
    expect(s.map((x) => [x.sid, x.title, x.node_ids])).toEqual([
      ['s1', 'Gamma', ['gamma']],
      ['s2', 'Alpha + Delta', ['alpha', 'delta', 'beta']],
    ]);
    expect(s[0]).toMatchObject({ status: 'starting', pid: null, current_node: null });
  });
  it('only nodes of the run count; titles are capped at 60 chars', () => {
    const g = updateNode(chainAndSolo(), 'alpha', { title: 'A'.repeat(80) });
    const s = groupSessions(g, ['alpha', 'beta']);
    expect(s).toHaveLength(1);
    expect(s[0]?.title).toHaveLength(60);
    expect(s[0]?.node_ids).toEqual(['alpha', 'beta']);
  });
});

describe('session status helpers', () => {
  const node = (status: NodeRunState['status'], started: boolean): NodeRunState => ({
    status, started_at: started ? 't' : null, finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: null,
  });
  const base = { sid: 's1', title: 't', node_ids: ['a', 'b'], status: 'starting' as const, pid: null, current_node: null };
  it('derives starting / running / done / cancelled', () => {
    expect(deriveSession(base, { a: node('queued', false), b: node('pending', false) }, false, 9).status).toBe('starting');
    expect(deriveSession(base, { a: node('running', true), b: node('pending', false) }, false, 9)).toMatchObject({ status: 'running', current_node: 'a', pid: 9 });
    expect(deriveSession(base, { a: node('succeeded', true), b: node('pending', false) }, false, 9)).toMatchObject({ status: 'running', current_node: null });
    expect(deriveSession(base, { a: node('succeeded', true), b: node('failed', true) }, false, 9).status).toBe('done');
    expect(deriveSession(base, { a: node('succeeded', true), b: node('cancelled', true) }, true, 9).status).toBe('cancelled');
  });
  it('alive = starting, running, or done with a live process', () => {
    expect(['starting', 'running', 'done', 'failed', 'cancelled'].map((status) => isSessionAlive({ status, pid: 7 } as never))).toEqual([true, true, true, false, false]);
    expect(isSessionAlive({ status: 'done', pid: null })).toBe(false);
    expect(isSessionAlive(null)).toBe(false);
    expect(sessionOfNode([base], 'b')?.sid).toBe('s1');
    expect(sessionOfNode([base], 'x')).toBeNull();
  });
  it('the typed message is the prompt verbatim when short and single-line, else a pointer to the brief', () => {
    const n = addNode(emptyGraph('g'), 'Write', { x: 0, y: 0 });
    const one = updateNode(n.graph, n.id, { prompt: 'say hi' }).nodes[0];
    const multi = updateNode(n.graph, n.id, { prompt: 'a\nb' }).nodes[0];
    if (!one || !multi) throw new Error('node missing');
    expect(terminalMessage(one, '/g')).toBe('say hi');
    expect(terminalMessage(multi, '/g/')).toBe(
      'Node "Write" (write): read the brief at /g/nodes/write/brief.md and do it; write all outputs to /g/nodes/write/deliverables.',
    );
  });
});

describe('mock terminal mode', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-11-07T10:15:00Z'));
  });
  afterEach(() => vi.useRealTimers());

  function setup(extra: Parameters<typeof createMockIpc>[0] = {}) {
    const opened: Array<[string, string]> = [];
    const api = createMockIpc({ stepMs: 100, ptyReplyMs: 50, trustAll: true, exampleRunMode: 'as_file', openWindow: (u, t) => opened.push([u, t]), ...extra });
    const states: RunState[] = [];
    const chunks: PtyData[] = [];
    void api.onRunState((s) => states.push(s));
    void api.onPtyData((d) => chunks.push(d));
    return { api, states, chunks, opened };
  }

  it('a run of the example is one session that streams a transcript and ends done', async () => {
    const { api, states, chunks } = setup();
    const runId = await api.startRun(MOCK_EXAMPLE_DIR, 'all');
    await vi.advanceTimersByTimeAsync(150);
    expect(last(states)?.mode).toBe('terminal');
    expect(sessionsOf(last(states))).toMatchObject([{ sid: 's1', status: 'running', current_node: 'survey', node_ids: ['survey', 'design', 'report'] }]);
    await vi.runAllTimersAsync();
    expect(last(states)?.status).toBe('succeeded');
    expect(sessionsOf(last(states))[0]?.status).toBe('done');
    const text = chunks.map((c) => c.data).join('');
    expect(chunks.every((c) => c.run_id === runId && c.sid === 's1')).toBe(true);
    expect(text).toContain('Claude Code (mock)');
    expect(text).toContain('read the brief at /mock/hello-pipeline/nodes/survey/brief.md');
    expect(chunks.map((c) => c.seq)).toEqual(chunks.map((_, i) => i + 1));
    expect(await api.ptyReplay(runId, 's1')).toEqual({ text, seq: chunks.length });
    expect(await api.listSessions(MOCK_EXAMPLE_DIR, runId)).toEqual(sessionsOf(last(states)));
    expect(await api.listSessions('', runId)).toHaveLength(1);
  });

  it('keystrokes echo, Enter gets a fake reply; bad sizes and unknown sessions are refused', async () => {
    const { api, chunks } = setup();
    const runId = await api.startRun(MOCK_EXAMPLE_DIR, 'all');
    await vi.runAllTimersAsync();
    chunks.length = 0;
    await api.ptyWrite(runId, 's1', 'hix\x7f\x1b[A\r');
    expect(chunks.map((c) => c.data).join('')).toBe('hix\b \b\r\n');
    await vi.advanceTimersByTimeAsync(60);
    expect(last(chunks)?.data).toContain('(mock) Received: hi');
    await expect(api.ptyResize(runId, 's1', 0, 40)).rejects.toMatchObject({ code: 'invalid_input' });
    await expect(api.ptyResize(runId, 's1', 120, 40)).resolves.toBeUndefined();
    await expect(api.ptyWrite(runId, 's9', 'x')).rejects.toMatchObject({ code: 'not_found' });
    await expect(api.ptyReplay('nope', 's1')).rejects.toMatchObject({ code: 'not_found' });
    await expect(api.listSessions('/other', runId)).rejects.toMatchObject({ code: 'not_found' });
  });

  it('open_session_window opens #/terminal/<run>/<sid> named term-<run>-<sid>', async () => {
    const { api, opened } = setup();
    const runId = await api.startRun(MOCK_EXAMPLE_DIR, 'all');
    await api.openSessionWindow(runId, 's1');
    expect(opened).toEqual([[`#/terminal/${runId}/s1?dir=%2Fmock%2Fhello-pipeline`, `term-${runId}-s1`]]);
    opened.length = 0;
    // Like the backend, the mock opens the window itself once the session starts (once).
    await vi.runAllTimersAsync();
    expect(opened).toEqual([[`#/terminal/${runId}/s1?dir=%2Fmock%2Fhello-pipeline`, `term-${runId}-s1`]]);
    await expect(api.openSessionWindow(runId, 's2')).rejects.toMatchObject({ code: 'not_found' });
  });

  it('headless graphs run without sessions or pty output', async () => {
    const { api, states, chunks } = setup({ exampleRunMode: 'headless' });
    await api.startRun(MOCK_EXAMPLE_DIR, 'all');
    await vi.runAllTimersAsync();
    expect(last(states)?.mode).toBe('headless');
    expect(sessionsOf(last(states))).toEqual([]);
    expect(chunks).toEqual([]);
  });

  it('a second tab sees the first tab\'s session over the channel', async () => {
    const a: MockChannel = { postMessage: (m) => b.onmessage?.({ data: structuredClone(m) }), onmessage: null };
    const b: MockChannel = { postMessage: (m) => a.onmessage?.({ data: structuredClone(m) }), onmessage: null };
    const main = setup({ channel: a });
    const tab = createMockIpc({ seed: false, channel: b });
    const tabChunks: PtyData[] = [];
    const tabStates: RunState[] = [];
    void tab.onPtyData((d) => tabChunks.push(d));
    void tab.onRunState((s) => tabStates.push(s));
    const runId = await main.api.startRun(MOCK_EXAMPLE_DIR, 'all');
    await vi.advanceTimersByTimeAsync(250);
    expect(tabChunks.length).toBeGreaterThan(0);
    expect(last(tabStates)?.run_id).toBe(runId);
    expect(await tab.listSessions('', runId)).toHaveLength(1);
    const replay = tab.ptyReplay(runId, 's1');
    await vi.advanceTimersByTimeAsync(0);
    expect(await replay).toEqual(await main.api.ptyReplay(runId, 's1'));
    main.chunks.length = 0;
    await tab.ptyWrite(runId, 's1', 'ok');
    expect(main.chunks.map((c) => c.data).join('')).toBe('ok');
    const unknown = tab.ptyReplay('other', 's1');
    await vi.advanceTimersByTimeAsync(500);
    expect(await unknown).toMatchObject({ seq: 0, text: expect.stringContaining('No tab of this browser runs other') });
  });

  it('a graph switched to headless in settings runs headless', async () => {
    const { api, states } = setup();
    const g = await api.openGraph(MOCK_EXAMPLE_DIR);
    await api.saveGraph(MOCK_EXAMPLE_DIR, updateSettings(g, { run_mode: 'headless' }));
    await api.startRun(MOCK_EXAMPLE_DIR, 'all');
    expect(last(states)?.mode).toBe('headless');
  });
});

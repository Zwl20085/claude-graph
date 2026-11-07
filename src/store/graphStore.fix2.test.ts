/** Fix pass 2 in the store: one denial toast per (run, node); ids follow titles until the node has run. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NodeEvent } from '../types/contract';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { createGraphStore } from './graphStore';

const DIR = MOCK_EXAMPLE_DIR;

function setup() {
  const api = createMockIpc({ stepMs: 10, trustAll: true });
  const store = createGraphStore(api, { autosaveMs: 60_000 });
  void api.onRunState((rs) => store.getState().applyRunState(rs));
  void api.onNodeEvent((ev) => store.getState().appendEvent(ev));
  return { api, store, s: () => store.getState() };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const denial = (run_id: string, node_id: string, text: string): NodeEvent => ({
  run_id, node_id, kind: 'permission_denied', text, ts: '2026-11-07T10:00:00Z',
});

describe('permission_denied toasts', () => {
  it('pushes one warning toast per node per run, on the first denial', async () => {
    const { s } = setup();
    await s().openGraph(DIR);
    s().updateNode('survey', { prompt: 'search FAKE_DENY' });
    expect(await s().startRun({ from: 'survey' })).toBe(true);
    const runId = s().runState?.run_id as string;
    s().appendEvent(denial(runId, 'survey', 'Bash: "ls"'));
    s().appendEvent(denial(runId, 'design', 'WebFetch: "https://x"'));
    await vi.runAllTimersAsync();
    const warnings = s().toasts.filter((t) => t.kind === 'warning').map((t) => t.text);
    expect(warnings).toEqual([
      'Node survey: Bash was denied. Set its permission mode to "Claude Code settings" or add the tool to its allowed tools.',
      'Node design: WebFetch was denied. Set its permission mode to "Claude Code settings" or add the tool to its allowed tools.',
    ]);
    expect(s().liveDenials.survey).toEqual(['Bash', 'WebSearch']);
    expect(s().events.survey?.some((l) => l.kind === 'permission_denied')).toBe(true);
  });

  it('a new run toasts again', async () => {
    const { s } = setup();
    await s().openGraph(DIR);
    s().updateNode('survey', { prompt: 'search FAKE_DENY' });
    await s().startRun({ from: 'survey' });
    await vi.runAllTimersAsync();
    await s().startRun({ from: 'survey' });
    await vi.runAllTimersAsync();
    expect(s().toasts.filter((t) => t.kind === 'warning')).toHaveLength(2);
  });
});

describe('ids follow the title (store)', () => {
  it('a new node follows, carrying selection; a hand-typed id stops it', async () => {
    const { s } = setup();
    await s().openGraph(DIR);
    const id = s().addNodeAt({ x: 0, y: 0 }) as string;
    expect(id).toBe('new-node');
    s().setTitle(id, 'Data prep');
    expect(s().selectedNodeId).toBe('data-prep');
    expect(s().freshNodeIds).toContain('data-prep');
    expect(s().renameNodeId('data-prep', 'mine')).toBeNull();
    s().setTitle('mine', 'Other title');
    expect(s().graph?.nodes.some((n) => n.id === 'mine' && n.title === 'Other title')).toBe(true);
  });

  it('no rename once the node has run; never-run nodes of an opened graph follow once runs are loaded', async () => {
    const { s, api } = setup();
    await s().openGraph(DIR);
    await s().startRun({ from: 'design' });
    await vi.runAllTimersAsync();
    // Reopen: the run list is loaded on entering the graph.
    const fresh = createGraphStore(api, { autosaveMs: 60_000 });
    await fresh.getState().openGraph(DIR);
    await vi.runAllTimersAsync();
    expect(fresh.getState().ranNodeIds).toEqual(['design', 'report']);
    fresh.getState().setTitle('design', 'Rules');
    expect(fresh.getState().graph?.nodes.find((n) => n.title === 'Rules')?.id).toBe('design');
    // survey never ran, but its id is hand-made ("survey" != slug("Literature survey")): kept.
    fresh.getState().setTitle('survey', 'Lit');
    expect(fresh.getState().graph?.nodes.find((n) => n.title === 'Lit')?.id).toBe('survey');
    // A never-run node whose id is slug(title) follows.
    fresh.getState().setTitle('survey', 'Survey');
    fresh.getState().setTitle('survey', 'Survey two');
    expect(fresh.getState().graph?.nodes.find((n) => n.title === 'Survey two')?.id).toBe('survey-two');
    expect(fresh.getState().graph?.edges.some((e) => e.from === 'survey-two' && e.to === 'design')).toBe(true);
  });
});

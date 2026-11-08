import { describe, expect, it, vi } from 'vitest';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { createMockAgentApi, type AgentApi } from '../lib/agentApi';
import type { AgentRunInfo } from '../types/agent';
import { createGraphStore } from './graphStore';
import { agentRunOf, createAgentStore, installAgentWatch, resetAgentStore } from './agentStore';

const info = (over: Partial<AgentRunInfo> = {}): AgentRunInfo => ({
  dir: 'C:/g',
  run_id: '20261108T000000Z',
  client: 'Claude Code',
  status: 'running',
  active: true,
  last_seen: '2026-11-08T00:00:00Z',
  last_note: null,
  ...over,
});

function fakeApi(over: Partial<AgentApi> = {}): AgentApi {
  return { ...createMockAgentApi(), ...over };
}

describe('agent store', () => {
  it('watches a folder and keeps only events of that folder', async () => {
    const api = fakeApi({ agentWatch: vi.fn(async () => info()) });
    const store = createAgentStore(api);
    await store.getState().watch('C:\\g');
    expect(store.getState().info?.client).toBe('Claude Code');
    store.getState().apply(info({ dir: 'C:/other', client: 'Codex' }));
    expect(store.getState().info?.client).toBe('Claude Code');
    store.getState().apply(info({ dir: 'c:/g/', client: 'Codex' }));
    expect(store.getState().info?.client).toBe('Codex');
    store.getState().apply({ nonsense: true });
    expect(store.getState().info?.client).toBe('Codex');
  });

  it('stop abandons the active run and reports errors', async () => {
    const abandon = vi.fn(async () => undefined);
    const store = createAgentStore(fakeApi({ agentWatch: async () => info(), agentAbandon: abandon }));
    await store.getState().watch('C:/g');
    expect(await store.getState().stop()).toBeNull();
    expect(abandon).toHaveBeenCalledWith('C:/g', '20261108T000000Z');
    expect(store.getState().info?.active).toBe(false);
    const failing = createAgentStore(
      fakeApi({ agentWatch: async () => info(), agentAbandon: async () => Promise.reject({ code: 'io', message: 'disk' }) }),
    );
    await failing.getState().watch('C:/g');
    expect(await failing.getState().stop()).toContain('disk');
  });

  it('re-watches when the graph store enters another folder', async () => {
    const watch = vi.fn(async () => null);
    const graph = createGraphStore(createMockIpc({ trustAll: true }), { autosaveMs: 60_000 });
    installAgentWatch(graph, fakeApi({ agentWatch: watch }));
    expect(watch).toHaveBeenLastCalledWith(null);
    await graph.getState().openGraph(MOCK_EXAMPLE_DIR);
    expect(watch).toHaveBeenLastCalledWith(MOCK_EXAMPLE_DIR);
    resetAgentStore();
  });

  it('agentRunOf matches the shown run only', () => {
    expect(agentRunOf(info(), '20261108T000000Z')?.client).toBe('Claude Code');
    expect(agentRunOf(info(), 'other')).toBeNull();
    expect(agentRunOf(null, 'x')).toBeNull();
    expect(agentRunOf(info(), undefined)).toBeNull();
  });
});

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createMockAgentApi } from '../lib/agentApi';
import { createAgentStore, resetAgentStore } from '../store/agentStore';
import { createGraphStore, installStore } from '../store/graphStore';
import { createMockIpc } from '../lib/ipc.mock';
import type { RunState } from '../types/contract';
import { AgentRunChip, drivenText } from './AgentRunChip';
import { CliAddonPanel } from './CliAddonPanel';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const RUN = '20261108T010203Z';

function runState(): RunState {
  return {
    run_id: RUN,
    graph_dir: 'C:/g',
    started_at: '2026-11-08T01:02:03Z',
    finished_at: null,
    status: 'running',
    nodes: { a: { status: 'running', started_at: null, finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: null } },
    mode: 'terminal',
    sessions: [],
  } as RunState;
}

describe('agent-called mode UI', () => {
  let root: Root | null = null;
  afterEach(async () => {
    if (root) await act(async () => root?.unmount());
    root = null;
    resetAgentStore();
    document.body.innerHTML = '';
  });

  const mount = async (node: React.ReactNode): Promise<HTMLElement> => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
    await act(async () => root?.render(node));
    return host;
  };

  it('chip names the client and Stop asks before abandoning', async () => {
    const graph = createGraphStore(createMockIpc({ trustAll: true }), { autosaveMs: 60_000 });
    installStore(graph);
    graph.setState({ dir: 'C:/g', runState: runState() });
    const abandon = vi.fn(async () => undefined);
    const agent = createAgentStore({
      ...createMockAgentApi(),
      agentWatch: async () => ({
        dir: 'C:/g', run_id: RUN, client: 'Codex', status: 'running', active: true,
        last_seen: '2026-11-08T01:02:03Z', last_note: 'halfway',
      }),
      agentAbandon: abandon,
    });
    resetAgentStore(agent);
    await agent.getState().watch('C:/g');
    const host = await mount(<AgentRunChip />);
    expect(host.textContent).toContain(drivenText('Codex', true));
    await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="stop the agent run"]')?.click());
    expect(abandon).not.toHaveBeenCalled();
    await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="confirm stop"]')?.click());
    expect(abandon).toHaveBeenCalledWith('C:/g', RUN);
    expect(host.textContent).toContain('(finished)');
  });

  it('settings panel shows snippets and asks before Add', async () => {
    const api = createMockAgentApi();
    const install = vi.spyOn(api, 'agentInstall');
    const host = await mount(<CliAddonPanel api={api} />);
    await act(async () => host.querySelector<HTMLButtonElement>('button[aria-expanded]')?.click());
    expect(host.textContent).toContain('claude mcp add --scope user claude-graph');
    await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="add to Codex"]')?.click());
    expect(install).not.toHaveBeenCalled();
    const confirm = host.querySelector('[aria-label="confirm add to Codex"]');
    expect(confirm?.textContent).toContain('codex mcp add claude_graph');
    await act(async () => confirm?.querySelector<HTMLButtonElement>('button')?.click());
    expect(install).toHaveBeenCalledWith('codex');
  });
});

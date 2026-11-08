/**
 * IPC of the agent-called mode (kept apart from `./api` so the core Api stays
 * untouched): `agent_watch`, `agent_abandon`, `agent_client_configs`,
 * `agent_install` and the `agent_run` event. In a plain browser (no Tauri) a
 * mock answers: no agent run, three example configs.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { isAppError, type AppError } from '../types/contract';
import { EVENT_AGENT_RUN, type AgentClientConfig, type AgentRunInfo } from '../types/agent';
import { isTauri } from './api';
import { describeError } from './ipc';

export interface AgentApi {
  /** Watch `dir` (null: stop); resolves to its newest agent run, if any. */
  agentWatch: (dir: string | null) => Promise<AgentRunInfo | null>;
  /** Stop: the agent run becomes cancelled. */
  agentAbandon: (dir: string, run_id: string) => Promise<void>;
  agentClientConfigs: () => Promise<AgentClientConfig[]>;
  /** Run the client's `mcp add` (or write dsh's patch file); call only after the user confirmed. */
  agentInstall: (client: string) => Promise<string>;
  onAgentRun: (handler: (info: AgentRunInfo) => void) => Promise<() => void>;
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (err) {
    if (isAppError(err)) throw err;
    const wrapped: AppError = { code: 'ipc', message: describeError(err) };
    throw wrapped;
  }
}

export const tauriAgentApi: AgentApi = {
  agentWatch: (dir) => call('agent_watch', { dir }),
  agentAbandon: (dir, run_id) => call('agent_abandon', { dir, run_id }),
  agentClientConfigs: () => call('agent_client_configs'),
  agentInstall: (client) => call('agent_install', { client }),
  onAgentRun: (handler) => listen<AgentRunInfo>(EVENT_AGENT_RUN, (e) => handler(e.payload)),
};

const MOCK_EXE = 'C:\\Program Files\\Claude Graph\\claude-graph.exe';

/** Browser-dev stand-in: never an agent run; installs succeed. */
export function createMockAgentApi(): AgentApi {
  const configs: AgentClientConfig[] = [
    {
      id: 'claude',
      name: 'Claude Code',
      snippet: `claude mcp add --scope user claude-graph -- "${MOCK_EXE}" --mcp-graph`,
      config_path: null,
      add_action: `run: claude mcp add --scope user claude-graph -- "${MOCK_EXE}" --mcp-graph`,
      installed: false,
    },
    {
      id: 'codex',
      name: 'Codex',
      snippet: `codex mcp add claude_graph -- "${MOCK_EXE}" --mcp-graph`,
      config_path: null,
      add_action: `run: codex mcp add claude_graph -- "${MOCK_EXE}" --mcp-graph`,
      installed: false,
    },
    {
      id: 'dsh',
      name: 'DeepSeek Harness (dsh)',
      snippet: '- insert:\n    - id: mcp-claude-graph\n',
      config_path: 'C:\\Users\\me\\.dsh\\cordis.patch.yml',
      add_action: 'add the entry to C:\\Users\\me\\.dsh\\cordis.patch.yml',
      installed: false,
    },
  ];
  return {
    agentWatch: async () => null,
    agentAbandon: async () => undefined,
    agentClientConfigs: async () => configs.map((c) => ({ ...c })),
    agentInstall: async (client) => `added claude-graph to ${client} (mock)`,
    onAgentRun: async () => () => undefined,
  };
}

export const agentApi: AgentApi = isTauri() ? tauriAgentApi : createMockAgentApi();

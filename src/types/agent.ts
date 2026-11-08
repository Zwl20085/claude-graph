/**
 * Agent-called mode (docs/cli-addon.md): a Claude Code / Codex / dsh session
 * drives a run through `claude-graph --mcp-graph` or `claude-graph graph …`.
 * Mirrors `src-tauri/src/contract_agent.rs`.
 */
import type { RunStatus } from './contract';

/** Event: the open folder's agent run changed. */
export const EVENT_AGENT_RUN = 'agent_run';

export interface AgentRunInfo {
  dir: string;
  run_id: string;
  /** Who drives it (`Claude Code`, `Codex`, `dsh`, …). */
  client: string;
  status: RunStatus;
  /** Still running: "Run all" is disabled and Stop is offered. */
  active: boolean;
  last_seen: string;
  /** Latest run-level note of the agent. */
  last_note: string | null;
}

/** One client's registration of the `--mcp-graph` server (Settings → "Use from your CLI"). */
export interface AgentClientConfig {
  id: 'claude' | 'codex' | 'dsh' | string;
  name: string;
  snippet: string;
  config_path: string | null;
  /** What "Add" does, shown in the confirmation. */
  add_action: string;
  installed: boolean;
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

export function isAgentRunInfo(v: unknown): v is AgentRunInfo {
  return (
    isObject(v) &&
    typeof v.dir === 'string' &&
    typeof v.run_id === 'string' &&
    typeof v.client === 'string' &&
    typeof v.status === 'string' &&
    typeof v.active === 'boolean'
  );
}

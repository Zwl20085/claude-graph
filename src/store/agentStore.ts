/**
 * The open folder's agent run (agent-called mode, docs/cli-addon.md): who
 * drives it and whether it is still active. Fed by `agent_watch` whenever the
 * graph store enters a folder and by `agent_run` events of that folder.
 */
import { createStore, useStore, type StoreApi } from 'zustand';
import { isAgentRunInfo, type AgentRunInfo } from '../types/agent';
import type { AgentApi } from '../lib/agentApi';
import type { GraphStore } from './graphStore';
import { errorMessage, samePath } from './runHelpers';

export interface AgentState {
  info: AgentRunInfo | null;
  /** The folder being watched. */
  dir: string | null;
  stopping: boolean;
}

export interface AgentActions {
  /** Watch `dir` (null: stop) and show its agent run. */
  watch: (dir: string | null) => Promise<void>;
  /** An `agent_run` event: kept only when it is about the watched folder. */
  apply: (info: unknown) => void;
  /** Stop button: the run becomes cancelled. Returns an error text or null. */
  stop: () => Promise<string | null>;
}

export type AgentStore = AgentState & AgentActions;

export function createAgentStore(api: AgentApi): StoreApi<AgentStore> {
  return createStore<AgentStore>()((set, get) => ({
    info: null,
    dir: null,
    stopping: false,
    watch: async (dir) => {
      set({ dir, info: null });
      try {
        const info = await api.agentWatch(dir);
        if (get().dir === dir) set({ info: isAgentRunInfo(info) ? info : null });
      } catch (err) {
        console.warn('agent_watch failed', err);
      }
    },
    apply: (info) => {
      if (!isAgentRunInfo(info) || !samePath(info.dir, get().dir)) return;
      set({ info });
    },
    stop: async () => {
      const { info, dir } = get();
      if (!info || !dir || !info.active) return null;
      set({ stopping: true });
      try {
        await api.agentAbandon(dir, info.run_id);
        set({ info: { ...info, active: false, status: 'cancelled' } });
        return null;
      } catch (err) {
        return errorMessage(err);
      } finally {
        set({ stopping: false });
      }
    },
  }));
}

let defaultStore: StoreApi<AgentStore> | null = null;

/** Bind the store to the graph store: re-watch on every folder change, listen for events. */
export function installAgentWatch(graph: StoreApi<GraphStore>, api: AgentApi): StoreApi<AgentStore> {
  const store = createAgentStore(api);
  defaultStore = store;
  let dir = graph.getState().dir;
  void store.getState().watch(dir);
  graph.subscribe((s) => {
    if (s.dir === dir) return;
    dir = s.dir;
    void store.getState().watch(dir);
  });
  api.onAgentRun((info) => store.getState().apply(info)).catch((err) =>
    graph.getState().pushToast('error', `Could not subscribe to agent runs: ${errorMessage(err)}`),
  );
  return store;
}

export function agentStore(): StoreApi<AgentStore> | null {
  return defaultStore;
}

const EMPTY: AgentStore = {
  info: null,
  dir: null,
  stopping: false,
  watch: async () => undefined,
  apply: () => undefined,
  stop: async () => null,
};
const fallback = createStore<AgentStore>()(() => EMPTY);

/** Selector hook; works (empty) before `installAgentWatch`, e.g. in tests. */
export function useAgentStore<T>(selector: (s: AgentStore) => T): T {
  return useStore(defaultStore ?? fallback, selector);
}

/** The agent run shown on the canvas, if the shown run is one. */
export function agentRunOf(info: AgentRunInfo | null, runId: string | null | undefined): AgentRunInfo | null {
  return info !== null && runId !== null && runId !== undefined && info.run_id === runId ? info : null;
}

/** Test hook: forget the installed store. */
export function resetAgentStore(store: StoreApi<AgentStore> | null = null): void {
  defaultStore = store;
}

/**
 * The single app store (zustand, vanilla core + React hook).
 *
 * Data integrity rules:
 * - Every graph change goes through `commit`, which replaces the graph object
 *   (graphOps are immutable) and schedules autosave (800 ms debounce).
 * - Saves are serialised on one promise chain so writes land in order; `dirty`
 *   clears only if the graph saved is still the current one.
 * - Switching or closing a graph saves first and aborts if that save fails.
 * - Before a run starts, pending edits are saved (the backend reads graph.json).
 */
import { createStore, useStore, type StoreApi } from 'zustand';
import type { ClaudeCheck, Graph, NodeEvent, Position, RunState, RunTarget, Settings } from '../types/contract';
import type { Api } from '../lib/api';
import { createDebouncer, defaultTimers, type Timers } from '../lib/debounce';
import * as ops from '../lib/graphOps';
import { isRunActive } from '../lib/status';
import { acceptsRunState, addEvent, errorMessage, newestFirst, type EventsByNode } from './runHelpers';

export const AUTOSAVE_MS = 800;
export const NEW_NODE_TITLE = 'New node';

export type ToastKind = 'info' | 'success' | 'error';
export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}
export type ClaudeState = { kind: 'checking' } | { kind: 'ok'; check: ClaudeCheck } | { kind: 'error'; message: string };

export interface GraphState {
  graph: Graph | null;
  dir: string | null;
  dirty: boolean;
  saving: boolean;
  saveError: string | null;
  /** Nodes created this session and not yet run: their id is still editable. */
  freshNodeIds: readonly string[];
  selectedNodeId: string | null;
  runState: RunState | null;
  startingRun: boolean;
  events: EventsByNode;
  eventsRunId: string | null;
  claude: ClaudeState;
  toasts: readonly Toast[];
  recent: readonly string[];
}

export interface GraphActions {
  init: () => Promise<void>;
  refreshRecent: () => Promise<void>;
  checkClaude: (bin?: string | null) => Promise<void>;
  openGraph: (dir: string) => Promise<boolean>;
  createGraph: (dir: string, name: string) => Promise<boolean>;
  closeGraph: () => Promise<boolean>;
  save: () => Promise<boolean>;
  setName: (name: string) => void;
  updateSettings: (patch: Partial<Settings>) => void;
  addNodeAt: (position: Position) => string | null;
  updateNode: (id: string, patch: ops.NodePatch) => void;
  renameNodeId: (oldId: string, newId: string) => string | null;
  removeElements: (nodeIds: readonly string[], edgeIds: readonly string[]) => void;
  connect: (from: string, to: string) => boolean;
  moveNodes: (positions: Readonly<Record<string, Position>>) => void;
  select: (id: string | null) => void;
  startRun: (target: RunTarget) => Promise<boolean>;
  cancelRun: () => Promise<void>;
  applyRunState: (rs: RunState) => void;
  appendEvent: (ev: NodeEvent) => void;
  clearEvents: (nodeId: string) => void;
  pushToast: (kind: ToastKind, text: string) => void;
  dismissToast: (id: number) => void;
}

export type GraphStore = GraphState & GraphActions;

const CLOSED: Pick<
  GraphState,
  'graph' | 'dir' | 'dirty' | 'saveError' | 'freshNodeIds' | 'selectedNodeId' | 'runState' | 'events' | 'eventsRunId'
> = {
  graph: null,
  dir: null,
  dirty: false,
  saveError: null,
  freshNodeIds: [],
  selectedNodeId: null,
  runState: null,
  events: {},
  eventsRunId: null,
};

/** Empty nodes the target would run; unknown targets check the whole graph. */
export function emptyTargets(graph: Graph, target: RunTarget, latest: RunState | null): string[] {
  let ids: string[];
  try {
    ids = ops.targetSet(graph, target, (id) => (latest?.run_id === id ? latest : undefined));
  } catch {
    ids = graph.nodes.map((n) => n.id);
  }
  return ops.emptyNodeIds(graph, ids);
}

export interface StoreOptions {
  autosaveMs?: number;
  timers?: Timers;
}

export function createGraphStore(api: Api, options: StoreOptions = {}): StoreApi<GraphStore> {
  return createStore<GraphStore>()((set, get) => {
    let toastSeq = 0;
    let lineSeq = 0;
    let chain: Promise<unknown> = Promise.resolve();

    const pushToast = (kind: ToastKind, text: string): void => {
      toastSeq += 1;
      set({ toasts: [...get().toasts, { id: toastSeq, kind, text }] });
    };

    const doSave = async (): Promise<boolean> => {
      const { graph, dir, dirty } = get();
      if (!graph || !dir || !dirty) return true;
      set({ saving: true });
      try {
        await api.saveGraph(dir, graph);
        set({ saving: false, saveError: null, dirty: get().graph === graph && get().dir === dir ? false : get().dirty });
        return true;
      } catch (err) {
        const message = errorMessage(err);
        if (message !== get().saveError) pushToast('error', `Save failed: ${message}`);
        set({ saving: false, saveError: message });
        return false;
      }
    };

    const save = (): Promise<boolean> => {
      autosave.cancel();
      const next = chain.then(doSave);
      chain = next.catch(() => undefined);
      return next;
    };

    const autosave = createDebouncer(() => void save(), options.autosaveMs ?? AUTOSAVE_MS, options.timers ?? defaultTimers);

    const commit = (next: Graph, extra: Partial<GraphState> = {}): void => {
      if (next === get().graph && Object.keys(extra).length === 0) return;
      set({ ...extra, graph: next, dirty: next === get().graph ? get().dirty : true });
      if (get().dirty) autosave.schedule();
    };

    const withGraph = (fn: (g: Graph) => void): void => {
      const g = get().graph;
      if (g) fn(g);
    };

    const loadLatestRun = async (dir: string): Promise<void> => {
      try {
        const latest = newestFirst(await api.listRuns(dir))[0];
        const rs = latest ? await api.getRun(dir, latest.run_id) : null;
        if (get().dir === dir) set({ runState: rs });
      } catch (err) {
        console.warn('could not load the latest run', err);
      }
    };

    const enter = (dir: string, graph: Graph): void => {
      set({ ...CLOSED, graph, dir });
      void loadLatestRun(dir);
      void get().refreshRecent();
      void get().checkClaude(graph.settings.claude_bin);
    };

    return {
      ...CLOSED,
      saving: false,
      startingRun: false,
      claude: { kind: 'checking' },
      toasts: [],
      recent: [],

      init: async () => {
        await Promise.all([get().refreshRecent(), get().checkClaude(null)]);
      },
      refreshRecent: async () => {
        try {
          set({ recent: await api.listRecent() });
        } catch (err) {
          console.warn('list_recent failed', err);
        }
      },
      checkClaude: async (bin) => {
        set({ claude: { kind: 'checking' } });
        try {
          set({ claude: { kind: 'ok', check: await api.checkClaude(bin ?? null) } });
        } catch (err) {
          set({ claude: { kind: 'error', message: errorMessage(err) } });
        }
      },
      openGraph: async (dir) => {
        if (!(await get().closeGraph())) return false;
        try {
          enter(dir, await api.openGraph(dir));
          return true;
        } catch (err) {
          pushToast('error', `Could not open ${dir}: ${errorMessage(err)}`);
          return false;
        }
      },
      createGraph: async (dir, name) => {
        if (!(await get().closeGraph())) return false;
        try {
          enter(dir, await api.createGraph(dir, name));
          return true;
        } catch (err) {
          pushToast('error', `Could not create graph in ${dir}: ${errorMessage(err)}`);
          return false;
        }
      },
      closeGraph: async () => {
        if (!(await save())) {
          pushToast('error', 'The open graph could not be saved; fix the error before switching.');
          return false;
        }
        set({ ...CLOSED });
        return true;
      },
      save,
      setName: (name) => withGraph((g) => commit({ ...g, name })),
      updateSettings: (patch) => {
        withGraph((g) => commit(ops.updateSettings(g, patch)));
        if ('claude_bin' in patch) void get().checkClaude(patch.claude_bin ?? null);
      },
      addNodeAt: (position) => {
        const g = get().graph;
        if (!g) return null;
        const { graph, id } = ops.addNode(g, NEW_NODE_TITLE, position);
        commit(graph, { selectedNodeId: id, freshNodeIds: [...get().freshNodeIds, id] });
        return id;
      },
      updateNode: (id, patch) => withGraph((g) => commit(ops.updateNode(g, id, patch))),
      renameNodeId: (oldId, newId) => {
        const g = get().graph;
        if (!g) return 'No graph open.';
        if (!get().freshNodeIds.includes(oldId)) return 'The id of a node that has been run cannot change.';
        const result = ops.renameNode(g, oldId, newId);
        if (!result.ok) return result.error;
        const swap = (id: string): string => (id === oldId ? newId : id);
        commit(result.graph, {
          freshNodeIds: get().freshNodeIds.map(swap),
          selectedNodeId: get().selectedNodeId === null ? null : swap(get().selectedNodeId as string),
          events: Object.fromEntries(Object.entries(get().events).map(([k, v]) => [swap(k), v])),
        });
        return null;
      },
      removeElements: (nodeIds, edgeIds) =>
        withGraph((g) => {
          const next = ops.removeElements(g, nodeIds, edgeIds);
          const sel = get().selectedNodeId;
          commit(next, {
            selectedNodeId: sel !== null && nodeIds.includes(sel) ? null : sel,
            freshNodeIds: get().freshNodeIds.filter((id) => !nodeIds.includes(id)),
          });
        }),
      connect: (from, to) => {
        const g = get().graph;
        if (!g) return false;
        const result = ops.connect(g, from, to);
        if (!result.ok) {
          pushToast('error', result.error);
          return false;
        }
        commit(result.graph);
        return true;
      },
      moveNodes: (positions) => withGraph((g) => commit(ops.applyPositions(g, positions))),
      select: (id) => set({ selectedNodeId: id }),

      startRun: async (target) => {
        const { dir, graph } = get();
        if (!dir || !graph) return false;
        if (isRunActive(get().runState)) {
          pushToast('error', 'A run is already active.');
          return false;
        }
        const empty = emptyTargets(graph, target, get().runState);
        if (empty.length > 0) {
          pushToast('error', `Refused: these nodes have no prompt or goal: ${empty.join(', ')}`);
          return false;
        }
        if (!(await save())) return false;
        set({ startingRun: true });
        try {
          const runId = await api.startRun(dir, target);
          set({ freshNodeIds: [] });
          if (get().runState?.run_id !== runId) set({ runState: await api.getRun(dir, runId) });
          return true;
        } catch (err) {
          pushToast('error', `Run failed to start: ${errorMessage(err)}`);
          return false;
        } finally {
          set({ startingRun: false });
        }
      },
      cancelRun: async () => {
        const rs = get().runState;
        if (!rs || !isRunActive(rs)) return;
        try {
          await api.cancelRun(rs.run_id);
        } catch (err) {
          pushToast('error', `Cancel failed: ${errorMessage(err)}`);
        }
      },
      applyRunState: (rs) => {
        const { dir, runState, startingRun } = get();
        if (acceptsRunState(rs, dir, runState, startingRun)) set({ runState: rs });
      },
      appendEvent: (ev) => {
        const { runState, startingRun, eventsRunId, events } = get();
        if (!startingRun && ev.run_id !== runState?.run_id) return;
        lineSeq += 1;
        const base = ev.run_id === eventsRunId ? events : {};
        set({ events: addEvent(base, ev, lineSeq), eventsRunId: ev.run_id });
      },
      clearEvents: (nodeId) => set({ events: { ...get().events, [nodeId]: [] } }),
      pushToast,
      dismissToast: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
    };
  });
}

let defaultStore: StoreApi<GraphStore> | null = null;

/** Bind the app-wide store (called once from main.tsx). */
export function installStore(store: StoreApi<GraphStore>): void {
  defaultStore = store;
}

export function graphStore(): StoreApi<GraphStore> {
  if (!defaultStore) throw new Error('graph store not installed');
  return defaultStore;
}

export function useGraphStore<T>(selector: (s: GraphStore) => T): T {
  return useStore(graphStore(), selector);
}

/**
 * The single app store (zustand, vanilla core + React hook).
 *
 * Data integrity rules:
 * - Every graph change goes through `commit`, which replaces the graph object
 *   (graphOps are immutable) and schedules autosave (800 ms debounce).
 * - Saves are serialised on one promise chain so writes land in order; `dirty`
 *   clears only if the graph saved is still the current one.
 * - Closing a graph saves until a save completes with nothing dirty (`saveAll`);
 *   opening/creating loads the new graph first, then saves and closes the old.
 * - Before a run starts, pending edits are saved (the backend reads graph.json)
 *   and the graph's elevated settings are trusted (trust dialog).
 * - Run state: live `run:state` events always win; fetched states (get_run)
 *   only fill a gap or replace a state that is strictly behind (H1, M4).
 *   While a start is pending, events of unknown runs are buffered until
 *   `start_run` names the new run id (M9).
 * - `init` honours `launch_options` once: open the folder, then `run all` if
 *   asked and claude was found (ARCHITECTURE.md "Launching").
 */
import { createStore, useStore, type StoreApi } from 'zustand';
import {
  isAppError,
  isRunState,
  type AppSettings,
  type ClaudeCheck,
  type Graph,
  type NodeEvent,
  type Position,
  type RunState,
  type RunTarget,
  type Settings,
  type TrustItem,
} from '../types/contract';
import type { Api } from '../lib/api';
import { createDebouncer, defaultTimers, type Timers } from '../lib/debounce';
import * as ops from '../lib/graphOps';
import { isRunActive } from '../lib/status';
import { createHarnessActions, INITIAL_HARNESSES, type HarnessActions, type HarnessState } from './harnessSlice';
import { createPermissionActions, INITIAL_PERMISSIONS, type PermissionActions, type PermissionState } from './permissionSlice';
import { answerFailed, createPromptActions, INITIAL_PROMPTS, type PromptActions, type PromptState } from './promptSlice';
import {
  acceptsRunState,
  addDenial,
  addEvent,
  deniedToast,
  deniedToolName,
  errorMessage,
  idIsEditable,
  LOG_CAP,
  newestFirst,
  preferFetched,
  unionIds,
  type DenialsByNode,
  type EventsByNode,
} from './runHelpers';

export const AUTOSAVE_MS = 800;
export const NEW_NODE_TITLE = 'New node';
/** closeGraph gives up after this many save rounds that each found new edits. */
export const MAX_SAVE_ROUNDS = 10;
export const IGNORED_CLAUDE_BIN_TEXT = 'ignored: claude path is a per-machine setting';

export type ToastKind = 'info' | 'success' | 'warning' | 'error';
export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
  /** A toast about a node: it offers "Show", which selects the node. */
  nodeId?: string;
}
export type ClaudeState = { kind: 'checking' } | { kind: 'ok'; check: ClaudeCheck } | { kind: 'error'; message: string };

export interface TrustRequest {
  dir: string;
  items: readonly TrustItem[];
}

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
  /** Tools denied live (permission_denied events) per node of `eventsRunId`. */
  liveDenials: DenialsByNode;
  /** Node ids that appear in any run of the open folder; null until loaded. */
  ranNodeIds: readonly string[] | null;
  claude: ClaudeState;
  appSettings: AppSettings | null;
  /** Shown as the trust dialog while non-null. */
  trustRequest: TrustRequest | null;
  toasts: readonly Toast[];
  recent: readonly string[];
}

export interface GraphActions {
  init: () => Promise<void>;
  launch: () => Promise<void>;
  refreshRecent: () => Promise<void>;
  checkClaude: () => Promise<void>;
  loadAppSettings: () => Promise<void>;
  /** Returns null on success, else the validation message to show inline. */
  saveAppSettings: (settings: AppSettings) => Promise<string | null>;
  openGraph: (dir: string) => Promise<boolean>;
  createGraph: (dir: string, name: string) => Promise<boolean>;
  closeGraph: () => Promise<boolean>;
  save: () => Promise<boolean>;
  /** Save until a save completes with nothing dirty (window close, graph switch). */
  saveAll: () => Promise<boolean>;
  setName: (name: string) => void;
  updateSettings: (patch: Partial<Settings>) => void;
  addNodeAt: (position: Position) => string | null;
  updateNode: (id: string, patch: ops.NodePatch) => void;
  /** Set the title; a never-run node whose id follows its title gets the new slug as id. */
  setTitle: (id: string, title: string) => void;
  renameNodeId: (oldId: string, newId: string) => string | null;
  removeElements: (nodeIds: readonly string[], edgeIds: readonly string[]) => void;
  connect: (from: string, to: string) => boolean;
  moveNodes: (positions: Readonly<Record<string, Position>>) => void;
  select: (id: string | null) => void;
  /** Resolves true when the graph in `dir` is trusted (asking the user if needed). */
  ensureTrusted: (dir: string) => Promise<boolean>;
  /** Answer of the trust dialog. */
  resolveTrust: (accept: boolean) => void;
  startRun: (target: RunTarget) => Promise<boolean>;
  cancelRun: () => Promise<void>;
  applyRunState: (rs: unknown) => void;
  appendEvent: (ev: NodeEvent) => void;
  clearEvents: (nodeId: string) => void;
  pushToast: (kind: ToastKind, text: string, nodeId?: string) => void;
  dismissToast: (id: number) => void;
}

export type GraphStore = GraphState &
  GraphActions &
  HarnessState &
  HarnessActions &
  PermissionState &
  PermissionActions &
  PromptState &
  PromptActions;

const CLOSED: Pick<
  GraphState,
  | 'graph'
  | 'dir'
  | 'dirty'
  | 'saveError'
  | 'freshNodeIds'
  | 'selectedNodeId'
  | 'runState'
  | 'events'
  | 'eventsRunId'
  | 'liveDenials'
  | 'ranNodeIds'
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
  liveDenials: {},
  ranNodeIds: null,
};

/** Empty nodes the target would run; unknown targets check the whole graph. */
export function emptyTargets(graph: Graph, target: RunTarget, latest: RunState | null): string[] {
  return ops.emptyNodeIds(graph, targetIds(graph, target, latest));
}

/** Node ids a target selects; the whole graph when the target cannot be resolved locally. */
export function targetIds(graph: Graph, target: RunTarget, latest: RunState | null): string[] {
  try {
    return ops.targetSet(graph, target, (id) => (latest?.run_id === id ? latest : undefined));
  } catch {
    return graph.nodes.map((n) => n.id);
  }
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
    /** Bumped whenever a graph is entered or closed; stale async results check it. */
    let epoch = 0;
    /** Settles when the latest run of the entered graph is loaded. */
    let entered: Promise<unknown> = Promise.resolve();
    let trustResolver: ((accept: boolean) => void) | null = null;
    /** run:state / node:event of unknown runs received while a start is pending. */
    let bufferedStates = new Map<string, RunState>();
    let bufferedEvents: NodeEvent[] = [];

    const pushToast = (kind: ToastKind, text: string, nodeId?: string): void => {
      toastSeq += 1;
      const toast: Toast = nodeId === undefined ? { id: toastSeq, kind, text } : { id: toastSeq, kind, text, nodeId };
      set({ toasts: [...get().toasts, toast] });
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

    const saveAll = async (): Promise<boolean> => {
      for (let round = 0; round < MAX_SAVE_ROUNDS; round += 1) {
        if (!(await save())) return false;
        if (!get().dirty) return true;
      }
      return false;
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

    const addLine = (ev: NodeEvent): void => {
      lineSeq += 1;
      const { eventsRunId, events, liveDenials } = get();
      const sameRun = ev.run_id === eventsRunId;
      const base = sameRun ? events : {};
      const denialsBase = sameRun ? liveDenials : {};
      if (ev.kind !== 'permission_denied') {
        set({ events: addEvent(base, ev, lineSeq), eventsRunId: ev.run_id, liveDenials: denialsBase });
        return;
      }
      const tool = deniedToolName(ev.text);
      const { denials, first } = addDenial(denialsBase, ev.node_id, tool);
      set({ events: addEvent(base, ev, lineSeq), eventsRunId: ev.run_id, liveDenials: denials });
      // One toast per node per run, on its first denial.
      if (first) pushToast('warning', deniedToast(ev.node_id, tool));
    };

    /** Show `rs`; buffered lines of that run are appended right away, in arrival order. */
    const showRun = (rs: RunState): void => {
      const ran = get().ranNodeIds;
      set({ runState: rs, ranNodeIds: ran === null ? null : unionIds(ran, Object.keys(rs.nodes)) });
      // Prompts of a waiting node show after a reload or an open too, not only on the next run:state.
      void get().syncSessionPrompts(rs);
      if (bufferedEvents.some((e) => e.run_id === rs.run_id)) {
        const mine = bufferedEvents.filter((e) => e.run_id === rs.run_id);
        bufferedEvents = bufferedEvents.filter((e) => e.run_id !== rs.run_id);
        mine.forEach(addLine);
      }
    };

    /** A get_run reply: only fills a gap or replaces a state that is strictly behind (H1). */
    const applyFetched = (dir: string, myEpoch: number, rs: RunState): void => {
      if (epoch !== myEpoch || get().dir !== dir) return;
      const current = get().runState;
      const next = preferFetched(current, rs);
      if (next !== current) showRun(next);
    };

    const loadLatestRun = async (dir: string, myEpoch: number): Promise<void> => {
      try {
        const latest = newestFirst(await api.listRuns(dir))[0];
        if (!latest) return;
        const rs = await api.getRun(dir, latest.run_id);
        // M4: a run started (or shown) meanwhile wins over the history lookup.
        const s = get();
        if (epoch === myEpoch && s.dir === dir && s.runState === null && !s.startingRun && isRunState(rs)) showRun(rs);
      } catch (err) {
        console.warn('could not load the latest run', err);
      }
    };

    /** Every node id of every run of `dir` (decides which ids may still follow their title). */
    const loadRanNodeIds = async (dir: string, myEpoch: number): Promise<void> => {
      try {
        const runs = await api.listRuns(dir);
        const states = await Promise.all(runs.map((r) => api.getRun(dir, r.run_id)));
        if (epoch !== myEpoch || get().dir !== dir) return;
        const ids = states.filter(isRunState).flatMap((rs) => Object.keys(rs.nodes));
        const shown = Object.keys(get().runState?.nodes ?? {});
        set({ ranNodeIds: unionIds(unionIds([], ids), shown) });
      } catch (err) {
        console.warn('could not load the node ids of past runs', err);
      }
    };

    /** State patch after node `oldId` became `newId`. */
    const renamedRefs = (oldId: string, newId: string): Partial<GraphState> => {
      const swap = (id: string): string => (id === oldId ? newId : id);
      const { freshNodeIds, selectedNodeId, events } = get();
      return {
        freshNodeIds: freshNodeIds.map(swap),
        selectedNodeId: selectedNodeId === null ? null : swap(selectedNodeId),
        events: Object.fromEntries(Object.entries(events).map(([k, v]) => [swap(k), v])),
      };
    };

    const dropTrustRequest = (): void => {
      const resolve = trustResolver;
      trustResolver = null;
      set({ trustRequest: null });
      resolve?.(false);
    };

    const closeNow = (): void => {
      epoch += 1;
      dropTrustRequest();
      set({ ...CLOSED, promptAnswering: [] });
    };

    const enter = (dir: string, graph: Graph): void => {
      epoch += 1;
      set({ ...CLOSED, promptAnswering: [], graph, dir });
      entered = loadLatestRun(dir, epoch);
      void loadRanNodeIds(dir, epoch);
      if (graph.settings.claude_bin !== null) pushToast('info', `graph.json sets claude_bin: ${IGNORED_CLAUDE_BIN_TEXT}.`);
      void get().refreshRecent();
    };

    /** Load a graph (new one first), then save and close the old one, then enter. */
    const switchTo = async (dir: string, load: () => Promise<Graph>, failText: string): Promise<boolean> => {
      let graph: Graph;
      try {
        graph = await load();
      } catch (err) {
        pushToast('error', `${failText}: ${errorMessage(err)}`);
        return false;
      }
      if (!(await get().closeGraph())) return false;
      enter(dir, graph);
      return true;
    };

    const startOnBackend = async (dir: string, target: RunTarget): Promise<string> => {
      try {
        return await api.startRun(dir, target);
      } catch (err) {
        // The trust status changed between our check and the start: ask once more, retry once.
        if (isAppError(err) && err.code === 'untrusted_graph' && (await get().ensureTrusted(dir))) {
          return api.startRun(dir, target);
        }
        throw err;
      }
    };

    return {
      ...CLOSED,
      ...INITIAL_HARNESSES,
      ...createHarnessActions(api, set, pushToast),
      ...INITIAL_PERMISSIONS,
      ...createPermissionActions(api, set, get, pushToast),
      ...INITIAL_PROMPTS,
      ...createPromptActions(api, set, get, pushToast),
      saving: false,
      startingRun: false,
      claude: { kind: 'checking' },
      appSettings: null,
      trustRequest: null,
      toasts: [],
      recent: [],

      init: async () => {
        await Promise.all([
          get().refreshRecent(),
          get().checkClaude(),
          get().loadAppSettings(),
          get().loadHarnesses(),
          get().loadPermissionPrompts(),
        ]);
        await get().launch();
      },
      launch: async () => {
        let opts;
        try {
          opts = await api.launchOptions();
        } catch (err) {
          console.warn('launch_options failed', err);
          return;
        }
        if (opts.open_dir === null || !(await get().openGraph(opts.open_dir))) return;
        if (!opts.autorun) return;
        await entered;
        const claude = get().claude;
        if (get().dir !== opts.open_dir) return;
        if (claude.kind !== 'ok' || !claude.check.found) {
          pushToast('error', 'Autorun skipped: claude was not found.');
          return;
        }
        await get().startRun('all');
      },
      refreshRecent: async () => {
        try {
          set({ recent: await api.listRecent() });
        } catch (err) {
          console.warn('list_recent failed', err);
        }
      },
      checkClaude: async () => {
        set({ claude: { kind: 'checking' } });
        try {
          set({ claude: { kind: 'ok', check: await api.checkClaude() } });
        } catch (err) {
          set({ claude: { kind: 'error', message: errorMessage(err) } });
        }
      },
      loadAppSettings: async () => {
        try {
          set({ appSettings: await api.getAppSettings() });
        } catch (err) {
          console.warn('get_app_settings failed', err);
        }
      },
      saveAppSettings: async (settings) => {
        try {
          set({ appSettings: await api.setAppSettings(settings) });
        } catch (err) {
          return errorMessage(err);
        }
        await get().checkClaude();
        return null;
      },
      openGraph: (dir) => switchTo(dir, () => api.openGraph(dir), `Could not open ${dir}`),
      createGraph: (dir, name) => switchTo(dir, () => api.createGraph(dir, name), `Could not create graph in ${dir}`),
      closeGraph: async () => {
        if (!(await saveAll())) {
          pushToast('error', 'The open graph could not be saved; fix the error before switching.');
          return false;
        }
        closeNow();
        return true;
      },
      save,
      saveAll,
      setName: (name) => withGraph((g) => commit({ ...g, name })),
      updateSettings: (patch) => withGraph((g) => commit(ops.updateSettings(g, patch))),
      addNodeAt: (position) => {
        const g = get().graph;
        if (!g) return null;
        const { graph, id } = ops.addNode(g, NEW_NODE_TITLE, position);
        commit(graph, { selectedNodeId: id, freshNodeIds: [...get().freshNodeIds, id] });
        return id;
      },
      updateNode: (id, patch) => withGraph((g) => commit(ops.updateNode(g, id, patch))),
      setTitle: (id, title) =>
        withGraph((g) => {
          const result = ops.retitleNode(g, id, title, idIsEditable(get(), id));
          commit(result.graph, result.id === id ? {} : renamedRefs(id, result.id));
        }),
      renameNodeId: (oldId, newId) => {
        const g = get().graph;
        if (!g) return 'No graph open.';
        if (!idIsEditable(get(), oldId)) return 'The id of a node that has been run cannot change.';
        const result = ops.renameNode(g, oldId, newId);
        if (!result.ok) return result.error;
        commit(result.graph, renamedRefs(oldId, newId));
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

      ensureTrusted: async (dir) => {
        let status;
        try {
          status = await api.graphTrustStatus(dir);
        } catch (err) {
          pushToast('error', `Could not check whether this graph is trusted: ${errorMessage(err)}`);
          return false;
        }
        if (status.trusted) return true;
        if (trustResolver !== null || get().dir !== dir) return false;
        const accepted = await new Promise<boolean>((resolve) => {
          trustResolver = resolve;
          set({ trustRequest: { dir, items: status.items } });
        });
        if (!accepted) return false;
        try {
          await api.trustGraph(dir);
          return true;
        } catch (err) {
          pushToast('error', `Could not trust the graph: ${errorMessage(err)}`);
          return false;
        }
      },
      resolveTrust: (accept) => {
        const resolve = trustResolver;
        trustResolver = null;
        set({ trustRequest: null });
        resolve?.(accept);
      },

      startRun: async (target) => {
        // M6: the guard and the flag are set synchronously, before any await.
        if (get().startingRun) return false;
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
        const myEpoch = epoch;
        const ids = targetIds(graph, target, get().runState);
        set({ startingRun: true });
        bufferedStates = new Map();
        bufferedEvents = [];
        try {
          if (!(await saveAll())) return false;
          if (!(await get().ensureTrusted(dir))) return false;
          if (epoch !== myEpoch) return false;
          const runId = await startOnBackend(dir, target);
          // L3: only the nodes this run targets lose their editable id.
          set({ freshNodeIds: get().freshNodeIds.filter((id) => !ids.includes(id)) });
          const early = bufferedStates.get(runId);
          if (early) applyFetched(dir, myEpoch, early);
          if (get().runState?.run_id !== runId) {
            try {
              const rs = await api.getRun(dir, runId);
              if (isRunState(rs)) applyFetched(dir, myEpoch, rs);
            } catch (err) {
              console.warn('get_run after start failed', err);
            }
          }
          return true;
        } catch (err) {
          pushToast('error', `Run failed to start: ${errorMessage(err)}`);
          return false;
        } finally {
          bufferedStates = new Map();
          bufferedEvents = [];
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
        if (!isRunState(rs)) {
          console.warn('ignored a malformed run:state payload', rs);
          return;
        }
        const { dir, runState, startingRun } = get();
        if (acceptsRunState(rs, dir, runState)) showRun(rs);
        else if (startingRun) bufferedStates = new Map(bufferedStates).set(rs.run_id, rs);
      },
      appendEvent: (ev) => {
        const { runState, startingRun } = get();
        if (answerFailed(ev, get().sessionPrompts, get().promptAnswering)) {
          get().releaseAnswers(ev.run_id, ev.node_id);
          pushToast('warning', ev.text, ev.node_id);
        }
        if (ev.run_id === runState?.run_id) addLine(ev);
        else if (startingRun) bufferedEvents = [...bufferedEvents, ev].slice(-LOG_CAP);
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

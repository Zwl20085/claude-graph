/**
 * Assistant-mode store (zustand), separate from the graph store.
 *
 * - `load(dir)` binds the store to one graph folder: fetches `assistant_state`
 *   and subscribes to `assistant_update` and `node:event`. The update payload
 *   names no folder, so any update triggers a debounced reload of our own dir.
 *   `close()` (or the next `load`) unsubscribes and drops pending reloads.
 * - Replies are applied in request order: a reply older than one already
 *   applied is dropped, as is any reply for a folder no longer bound.
 * - `send` saves the graph first (the scheduler reads graph.json from disk);
 *   a failed save blocks the send with a toast.
 * - Every api failure is reported through the graph store's toasts.
 */
import { createStore, useStore, type StoreApi } from 'zustand';
import {
  ASSISTANT_RUN_ID,
  isAssistantState,
  type AssistantState,
  type NodeEvent,
} from '../types/contract';
import { api, type Api, type Unlisten } from '../lib/api';
import { createDebouncer, defaultTimers, type Timers } from '../lib/debounce';
import { graphStore, type GraphStore } from './graphStore';
import { appendCapped, errorMessage, LOG_CAP, type LogLine } from './runHelpers';

export const ASSISTANT_RELOAD_MS = 100;
export const RUN_WORKFLOW_TEXT = 'Execute the workflow from the start.';

export interface AssistantStoreState {
  dir: string | null;
  state: AssistantState | null;
  /** A send / cancel / reset call is in flight. */
  busy: boolean;
  /** Last failure to load the state of `dir` (null when the last load worked). */
  error: string | null;
  /** Live scheduler lines (`node:event` with run_id "assistant") of this session. */
  log: readonly LogLine[];
}

export interface AssistantActions {
  load: (dir: string) => Promise<void>;
  /** Unbind: unsubscribe, drop pending reloads, clear the state. */
  close: () => void;
  /** Resolves true when the backend accepted the message. */
  send: (text: string) => Promise<boolean>;
  cancel: () => Promise<void>;
  reset: () => Promise<boolean>;
  clearLog: () => void;
}

export type AssistantStore = AssistantStoreState & AssistantActions;

export interface AssistantStoreOptions {
  reloadMs?: number;
  timers?: Timers;
}

const CLEARED: AssistantStoreState = { dir: null, state: null, busy: false, error: null, log: [] };

export function createAssistantStore(
  api: Api,
  graph: () => StoreApi<GraphStore>,
  options: AssistantStoreOptions = {},
): StoreApi<AssistantStore> {
  return createStore<AssistantStore>()((set, get) => {
    /** Bumped on every load/close; stale async work compares against it. */
    let epoch = 0;
    let requestSeq = 0;
    let appliedSeq = 0;
    let lineSeq = 0;
    let unlisteners: Unlisten[] = [];

    const toast = (text: string): void => graph().getState().pushToast('error', text);

    /** Apply a reply if it is for the bound folder and newer than the last applied one. */
    const apply = (myEpoch: number, seq: number, value: unknown): void => {
      if (myEpoch !== epoch || seq <= appliedSeq) return;
      if (!isAssistantState(value)) {
        console.warn('ignored a malformed assistant state', value);
        return;
      }
      appliedSeq = seq;
      set({ state: value, error: null });
    };

    const reload = async (): Promise<void> => {
      const { dir } = get();
      if (dir === null) return;
      const myEpoch = epoch;
      requestSeq += 1;
      const seq = requestSeq;
      try {
        apply(myEpoch, seq, await api.assistantState(dir));
      } catch (err) {
        if (myEpoch !== epoch) return;
        const message = errorMessage(err);
        if (message !== get().error) toast(`Could not load the assistant: ${message}`);
        set({ error: message });
      }
    };

    const reloader = createDebouncer(() => void reload(), options.reloadMs ?? ASSISTANT_RELOAD_MS, options.timers ?? defaultTimers);

    const onEvent = (ev: NodeEvent): void => {
      if (ev.run_id !== ASSISTANT_RUN_ID) return;
      lineSeq += 1;
      set({ log: appendCapped(get().log, { seq: lineSeq, kind: ev.kind, text: ev.text, ts: ev.ts }, LOG_CAP) });
    };

    const unsubscribe = (): void => {
      reloader.cancel();
      unlisteners.forEach((off) => off());
      unlisteners = [];
    };

    /** Subscribe; a subscription that resolves after the folder changed is dropped at once. */
    const subscribe = (myEpoch: number): void => {
      const keep = (off: Unlisten): void => {
        if (myEpoch === epoch) unlisteners = [...unlisteners, off];
        else off();
      };
      const failed = (name: string) => (err: unknown) =>
        toast(`Could not subscribe to ${name}; assistant updates will be missing: ${errorMessage(err)}`);
      api.onAssistantUpdate(() => reloader.schedule()).then(keep, failed('assistant_update'));
      api.onNodeEvent(onEvent).then(keep, failed('node:event'));
    };

    /** Common wrapper of send / cancel / reset: one call at a time, bound folder only. */
    const guarded = async <T>(fn: (dir: string, myEpoch: number) => Promise<T>, fallback: T): Promise<T> => {
      const { dir, busy } = get();
      if (dir === null || busy) return fallback;
      set({ busy: true });
      const myEpoch = epoch;
      try {
        return await fn(dir, myEpoch);
      } finally {
        if (myEpoch === epoch) set({ busy: false });
      }
    };

    return {
      ...CLEARED,

      load: async (dir) => {
        unsubscribe();
        epoch += 1;
        set({ ...CLEARED, dir });
        subscribe(epoch);
        await reload();
      },

      close: () => {
        unsubscribe();
        epoch += 1;
        set({ ...CLEARED });
      },

      send: (raw) =>
        guarded(async (dir, myEpoch) => {
          const text = raw.trim();
          if (text === '') return false;
          if (get().state?.session.status === 'working') {
            toast('The scheduler is working; wait for its reply or cancel the turn.');
            return false;
          }
          if (!(await graph().getState().save())) {
            toast('Not sent: the graph could not be saved, and the scheduler reads graph.json from disk.');
            return false;
          }
          if (myEpoch !== epoch) return false;
          requestSeq += 1;
          const seq = requestSeq;
          try {
            apply(myEpoch, seq, await api.assistantSend(dir, text));
            return true;
          } catch (err) {
            toast(`Not sent: ${errorMessage(err)}`);
            return false;
          }
        }, false),

      cancel: () =>
        guarded(async (dir) => {
          try {
            await api.assistantCancel(dir);
          } catch (err) {
            toast(`Cancel failed: ${errorMessage(err)}`);
          }
          reloader.schedule();
        }, undefined),

      reset: () =>
        guarded(async (dir, myEpoch) => {
          requestSeq += 1;
          const seq = requestSeq;
          try {
            apply(myEpoch, seq, await api.assistantReset(dir));
            return true;
          } catch (err) {
            toast(`Reset failed: ${errorMessage(err)}`);
            return false;
          }
        }, false),

      clearLog: () => set({ log: [] }),
    };
  });
}

let defaultStore: StoreApi<AssistantStore> | null = null;

/** Bind the app-wide assistant store (called once from main.tsx). */
export function installAssistantStore(store: StoreApi<AssistantStore>): void {
  defaultStore = store;
}

/** The app-wide store; created on first use against `api` and the installed graph store. */
export function assistantStore(): StoreApi<AssistantStore> {
  if (!defaultStore) defaultStore = createAssistantStore(api, graphStore);
  return defaultStore;
}

export function useAssistantStore<T>(selector: (s: AssistantStore) => T): T {
  return useStore(assistantStore(), selector);
}

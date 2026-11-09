/**
 * The Inbox panel's state (docs/INBOX.md): the backend's `InboxStatus` (fed
 * by `inbox_status`, `inbox_scan_now` and `inbox:update`) and the per-machine
 * inbox settings, saved through `set_app_settings` together with the other
 * app settings so nothing else is reset.
 */
import { createStore, useStore, type StoreApi } from 'zustand';
import {
  DEFAULT_INBOX_SETTINGS,
  type AppSettings,
  type InboxSettings,
  type InboxStatus,
} from '../types/contract';
import type { Api } from '../lib/api';
import { clampInbox, type InboxApi } from '../lib/inboxApi';
import { errorMessage } from './runHelpers';

type SettingsApi = Pick<Api, 'getAppSettings' | 'setAppSettings' | 'pickFolder' | 'openSessionWindow'>;

export interface InboxState {
  open: boolean;
  status: InboxStatus | null;
  /** The app settings as last loaded or saved (null: not loaded yet). */
  app: AppSettings | null;
  busy: boolean;
  error: string | null;
}

export interface InboxActions {
  setOpen: (open: boolean) => void;
  /** Load the status and the settings. */
  load: () => Promise<void>;
  /** An `inbox:update` payload. */
  apply: (status: InboxStatus) => void;
  /** Merge `patch` into the inbox settings and save; returns an error text or null. */
  saveInbox: (patch: Partial<InboxSettings>) => Promise<string | null>;
  saveAutostart: (on: boolean) => Promise<string | null>;
  /** Folder dialog; the picked folder is saved (not enabled). */
  pickFolder: () => Promise<string | null>;
  scanNow: () => Promise<string | null>;
  openSession: (tipId: string) => Promise<string | null>;
  revealResults: (tipId: string) => Promise<string | null>;
}

export type InboxStore = InboxState & InboxActions;

export function inboxSettingsOf(app: AppSettings | null): InboxSettings {
  return { ...DEFAULT_INBOX_SETTINGS, ...(app?.inbox ?? {}) };
}

/** `onSaved`: the stored app settings after every save (keeps other copies fresh). */
export function createInboxStore(
  api: SettingsApi,
  inbox: InboxApi,
  onSaved: (settings: AppSettings) => void = () => undefined,
): StoreApi<InboxStore> {
  return createStore<InboxStore>()((set, get) => {
    const save = async (next: AppSettings): Promise<string | null> => {
      set({ busy: true });
      try {
        const stored = await api.setAppSettings(next);
        set({ app: stored, error: null });
        onSaved(stored);
        set({ status: await inbox.inboxStatus() });
        return null;
      } catch (err) {
        const msg = errorMessage(err);
        set({ error: msg });
        return msg;
      } finally {
        set({ busy: false });
      }
    };
    // Always the backend's copy: another panel may have saved meanwhile.
    const current = (): Promise<AppSettings> => api.getAppSettings();
    const guard = async (f: () => Promise<void>): Promise<string | null> => {
      try {
        await f();
        return null;
      } catch (err) {
        const msg = errorMessage(err);
        set({ error: msg });
        return msg;
      }
    };
    return {
      open: false,
      status: null,
      app: null,
      busy: false,
      error: null,
      setOpen: (open) => {
        set({ open });
        if (open) void get().load();
      },
      load: async () => {
        await guard(async () => {
          const [status, app] = await Promise.all([inbox.inboxStatus(), api.getAppSettings()]);
          set({ status, app, error: null });
        });
      },
      apply: (status) => set({ status }),
      saveInbox: async (patch) => {
        const app = await current();
        return save({ ...app, inbox: clampInbox({ ...inboxSettingsOf(app), ...patch }) });
      },
      saveAutostart: async (on) => save({ ...(await current()), autostart: on }),
      pickFolder: async () => {
        let picked: string | null = null;
        const err = await guard(async () => {
          picked = await api.pickFolder('Choose the Vindictive tips folder');
        });
        if (err !== null) return err;
        return picked === null ? null : get().saveInbox({ folder: picked });
      },
      scanNow: async () =>
        guard(async () => {
          set({ status: await inbox.inboxScanNow(), error: null });
        }),
      openSession: async (tipId) =>
        guard(async () => {
          const run = get().status?.tips.find((t) => t.id === tipId)?.run;
          if (!run?.graph_run_id || !run.sid) throw { code: 'not_found', message: 'no session for this tip yet' };
          await api.openSessionWindow(run.graph_run_id, run.sid);
        }),
      revealResults: async (tipId) => guard(() => inbox.inboxRevealResults(tipId)),
    };
  });
}

let defaultStore: StoreApi<InboxStore> | null = null;

/** Create the app's store and subscribe it to `inbox:update`. */
export function installInboxStore(
  api: SettingsApi,
  inbox: InboxApi,
  onListenError: (text: string) => void,
  onSaved?: (settings: AppSettings) => void,
): StoreApi<InboxStore> {
  const store = createInboxStore(api, inbox, onSaved);
  defaultStore = store;
  inbox
    .onInboxUpdate((s) => store.getState().apply(s))
    .catch((err) => onListenError(`Could not subscribe to inbox updates: ${errorMessage(err)}`));
  return store;
}

const EMPTY: InboxStore = {
  open: false,
  status: null,
  app: null,
  busy: false,
  error: null,
  setOpen: () => undefined,
  load: async () => undefined,
  apply: () => undefined,
  saveInbox: async () => null,
  saveAutostart: async () => null,
  pickFolder: async () => null,
  scanNow: async () => null,
  openSession: async () => null,
  revealResults: async () => null,
};
const fallback = createStore<InboxStore>()(() => EMPTY);

/** Selector hook; works (empty) before `installInboxStore`, e.g. in tests. */
export function useInboxStore<T>(selector: (s: InboxStore) => T): T {
  return useStore(defaultStore ?? fallback, selector);
}

export function inboxStore(): StoreApi<InboxStore> | null {
  return defaultStore;
}

/** Test hook: install (or forget) a store. */
export function resetInboxStore(store: StoreApi<InboxStore> | null = null): void {
  defaultStore = store;
}

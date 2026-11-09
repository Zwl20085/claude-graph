/**
 * IPC of the Vindictive inbox (docs/INBOX.md), kept apart from `./api` like
 * the agent-called mode: `inbox_status`, `inbox_scan_now`,
 * `inbox_reveal_results` and the `inbox:update` event. Settings go through
 * the core `get_app_settings` / `set_app_settings`. In a plain browser a mock
 * answers (see `createMockInboxApi`).
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  DEFAULT_INBOX_SETTINGS,
  EVENT_INBOX_UPDATE,
  isAppError,
  type AppError,
  type InboxSettings,
  type InboxStatus,
  type InboxTip,
} from '../types/contract';
import { isTauri } from './api';
import { describeError } from './ipc';
import { clampInbox } from './inboxSettings';

export interface InboxApi {
  inboxStatus: () => Promise<InboxStatus>;
  /** Rejects with invalid_input when the inbox is off. */
  inboxScanNow: () => Promise<InboxStatus>;
  /** Reveal `<tips folder>/agent/<tip id>/` (not_found before the first result). */
  inboxRevealResults: (tip_id: string) => Promise<void>;
  onInboxUpdate: (handler: (status: InboxStatus) => void) => Promise<() => void>;
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

export const tauriInboxApi: InboxApi = {
  inboxStatus: () => call('inbox_status'),
  inboxScanNow: () => call('inbox_scan_now'),
  inboxRevealResults: (tip_id) => call('inbox_reveal_results', { tip_id }),
  onInboxUpdate: (handler) => listen<InboxStatus>(EVENT_INBOX_UPDATE, (e) => handler(e.payload)),
};

export { clampInbox };

export function emptyInboxStatus(settings: InboxSettings = DEFAULT_INBOX_SETTINGS): InboxStatus {
  return {
    enabled: settings.enabled,
    folder: settings.folder,
    suggested_folder: settings.folder === '' ? 'C:\\Users\\me\\OneDrive\\Vindictive' : null,
    host: 'MOCK-PC',
    scanning: false,
    last_scan: null,
    next_scan: null,
    last_error: null,
    tips: [],
    skipped: [],
    other_tips: 0,
    log: [],
  };
}

export interface MockInboxOptions {
  /** The inbox settings as the mock backend has them. */
  settings?: () => InboxSettings;
  tips?: InboxTip[];
  now?: () => Date;
}

/** Browser-dev / test stand-in: a scan lists the given tips; nothing runs. */
export function createMockInboxApi(options: MockInboxOptions = {}): InboxApi {
  const settings = options.settings ?? (() => DEFAULT_INBOX_SETTINGS);
  const listeners = new Set<(s: InboxStatus) => void>();
  let lastScan: string | null = null;
  const status = (): InboxStatus => ({
    ...emptyInboxStatus(settings()),
    last_scan: lastScan,
    tips: lastScan === null ? [] : (options.tips ?? []).map((t) => ({ ...t })),
  });
  return {
    inboxStatus: async () => status(),
    inboxScanNow: async () => {
      if (!settings().enabled) {
        const err: AppError = { code: 'invalid_input', message: 'the inbox is off; enable it and choose the tips folder first' };
        throw err;
      }
      lastScan = (options.now ?? (() => new Date()))().toISOString();
      const s = status();
      listeners.forEach((h) => h(s));
      return s;
    },
    inboxRevealResults: async (tipId) => {
      if (!(options.tips ?? []).some((t) => t.id === tipId)) {
        const err: AppError = { code: 'not_found', message: `no agent tip "${tipId}"` };
        throw err;
      }
    },
    onInboxUpdate: async (handler) => {
      listeners.add(handler);
      return () => listeners.delete(handler);
    },
  };
}

export const inboxApi: InboxApi = isTauri() ? tauriInboxApi : createMockInboxApi();

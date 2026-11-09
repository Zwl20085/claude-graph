import { describe, expect, it, vi } from 'vitest';
import { createMockIpc } from '../lib/ipc.mock';
import { clampInbox, createMockInboxApi, emptyInboxStatus, type InboxApi } from '../lib/inboxApi';
import { DEFAULT_INBOX_SETTINGS, type InboxSettings, type InboxTip } from '../types/contract';
import { createInboxStore, inboxSettingsOf, installInboxStore, resetInboxStore } from './inboxStore';

const TIP: InboxTip = {
  id: 'survey',
  title: 'Survey papers',
  state: 'question',
  decision: 'unchanged since question',
  eligible: false,
  host: 'MOCK-PC',
  at: '2026-11-09T07:31:00',
  run: {
    run: '20261109-073100-ab12',
    phase: 'question',
    graph_dir: 'C:/data/inbox/survey-1234abcd',
    graph_run_id: '20261109T063100Z',
    sid: 's1',
    started_at: '2026-11-09T06:31:00Z',
    note: 'question written',
  },
};

function setup(over: Partial<InboxApi> = {}) {
  const ipc = createMockIpc({ seed: false, pickFolder: () => 'C:/tips' });
  let settings: InboxSettings = DEFAULT_INBOX_SETTINGS;
  const mock = createMockInboxApi({ settings: () => settings, tips: [TIP] });
  const inbox: InboxApi = { ...mock, ...over };
  const setAppSettings = vi.fn(async (s: Parameters<typeof ipc.setAppSettings>[0]) => {
    const stored = await ipc.setAppSettings(s);
    settings = inboxSettingsOf(stored);
    return stored;
  });
  const openSessionWindow = vi.fn(async () => undefined);
  const saved = vi.fn();
  const store = createInboxStore({ ...ipc, setAppSettings, openSessionWindow }, inbox, saved);
  return { store, s: () => store.getState(), setAppSettings, openSessionWindow, saved, ipc };
}

describe('inbox store', () => {
  it('loads status and settings, with defaults for older settings files', async () => {
    const { store, s } = setup();
    expect(inboxSettingsOf(null)).toEqual(DEFAULT_INBOX_SETTINGS);
    s().setOpen(true);
    await vi.waitFor(() => expect(s().status).not.toBeNull());
    expect(store.getState().open).toBe(true);
    expect(s().status?.suggested_folder).toContain('Vindictive');
    expect(inboxSettingsOf(s().app).enabled).toBe(false);
  });

  it('saves inbox settings without dropping the claude path, clamped', async () => {
    const { s, setAppSettings, saved, ipc } = setup();
    await ipc.setAppSettings({ claude_bin: '/opt/claude' });
    expect(await s().saveInbox({ interval_hours: 500, max_parallel: 0 })).toBeNull();
    const sent = setAppSettings.mock.calls[0]?.[0];
    if (!sent) throw new Error('not sent');
    expect(sent.claude_bin).toBe('/opt/claude');
    expect(sent.inbox).toMatchObject({ interval_hours: 168, max_parallel: 1 });
    expect(saved).toHaveBeenCalledWith(expect.objectContaining({ claude_bin: '/opt/claude' }));
    expect(await s().saveAutostart(true)).toBeNull();
    expect(s().app).toMatchObject({ autostart: true, inbox: { interval_hours: 168 } });
  });

  it('enabling needs a folder; the picker saves the folder', async () => {
    const { s } = setup();
    expect(await s().saveInbox({ enabled: true })).toMatch(/folder/);
    expect(s().error).toMatch(/folder/);
    expect(await s().pickFolder()).toBeNull();
    expect(inboxSettingsOf(s().app).folder).toBe('C:/tips');
    expect(await s().saveInbox({ enabled: true })).toBeNull();
    expect(s().status?.enabled).toBe(true);
  });

  it('scan now refuses when off, lists tips when on, and opens the session', async () => {
    const { s, openSessionWindow } = setup();
    expect(await s().scanNow()).toMatch(/inbox is off/);
    await s().pickFolder();
    await s().saveInbox({ enabled: true });
    expect(await s().scanNow()).toBeNull();
    expect(s().status?.tips[0]?.id).toBe('survey');
    expect(s().status?.last_scan).not.toBeNull();
    expect(await s().openSession('survey')).toBeNull();
    expect(openSessionWindow).toHaveBeenCalledWith('20261109T063100Z', 's1');
    expect(await s().openSession('nope')).toMatch(/no session/);
    expect(await s().revealResults('survey')).toBeNull();
    expect(await s().revealResults('nope')).toMatch(/no agent tip/);
  });

  it('applies inbox:update events and reports a failed subscription', async () => {
    const handlers: Array<(st: ReturnType<typeof emptyInboxStatus>) => void> = [];
    const store = installInboxStore(
      { ...createMockIpc({ seed: false }) },
      { ...createMockInboxApi(), onInboxUpdate: async (h) => (handlers.push(h), () => undefined) },
      () => undefined,
    );
    handlers[0]?.({ ...emptyInboxStatus(), host: 'X', tips: [TIP] });
    expect(store.getState().status?.host).toBe('X');
    const errors: string[] = [];
    installInboxStore(
      createMockIpc({ seed: false }),
      { ...createMockInboxApi(), onInboxUpdate: async () => Promise.reject(new Error('nope')) },
      (t) => errors.push(t),
    );
    await vi.waitFor(() => expect(errors[0]).toContain('inbox updates'));
    resetInboxStore();
  });

  it('load errors are kept', async () => {
    const { s } = setup({ inboxStatus: async () => Promise.reject({ code: 'io', message: 'disk' }) });
    await s().load();
    expect(s().error).toContain('disk');
  });

  it('clampInbox matches the backend limits', () => {
    expect(clampInbox({ ...DEFAULT_INBOX_SETTINGS, folder: ' C:/x ', timeout_minutes: 1, interval_hours: 0.4 })).toMatchObject({
      folder: 'C:/x',
      timeout_minutes: 5,
      interval_hours: 1,
    });
  });
});

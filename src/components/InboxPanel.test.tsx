import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createMockIpc } from '../lib/ipc.mock';
import { createMockInboxApi, emptyInboxStatus } from '../lib/inboxApi';
import { createInboxStore, resetInboxStore } from '../store/inboxStore';
import { createGraphStore, installStore } from '../store/graphStore';
import type { InboxTip } from '../types/contract';
import { InboxButton, InboxDialog, InboxPanel, STATE_BADGES, when } from './InboxPanel';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const tip = (over: Partial<InboxTip> = {}): InboxTip => ({
  id: 'survey',
  title: 'Survey papers',
  state: 'question',
  decision: 'unchanged since question',
  eligible: false,
  host: 'MOCK-PC',
  at: '2026-11-09T07:31:00',
  run: {
    run: 'r1',
    phase: 'question',
    graph_dir: 'C:/data/inbox/survey-1234abcd',
    graph_run_id: '20261109T063100Z',
    sid: 's1',
    started_at: null,
    note: 'question written',
  },
  ...over,
});

describe('Inbox panel', () => {
  let root: Root | null = null;
  afterEach(async () => {
    if (root) await act(async () => root?.unmount());
    root = null;
    resetInboxStore();
    document.body.innerHTML = '';
  });

  const mount = async (node: React.ReactNode): Promise<HTMLElement> => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
    await act(async () => root?.render(node));
    return host;
  };

  const q = <T extends Element = HTMLButtonElement>(host: HTMLElement, label: string): T | null =>
    host.querySelector<T>(`[aria-label="${label}"]`);

  it('lists tips with badges, run state, skipped files and works the buttons', async () => {
    const graph = createGraphStore(createMockIpc({ trustAll: true }), { autosaveMs: 60_000 });
    installStore(graph);
    const openGraph = vi.fn(async (_dir: string) => true);
    graph.setState({ openGraph });
    const ipc = createMockIpc({ seed: false });
    const openSessionWindow = vi.fn(async () => undefined);
    const store = createInboxStore({ ...ipc, openSessionWindow }, createMockInboxApi());
    resetInboxStore(store);
    store.setState({
      open: true,
      app: { claude_bin: null },
      status: {
        ...emptyInboxStatus(),
        enabled: true,
        folder: 'C:/tips',
        suggested_folder: null,
        last_scan: '2026-11-09T07:00:00Z',
        tips: [
          tip(),
          tip({ id: 'new', title: 'New one', state: 'queued', decision: 'new task', eligible: true, run: null, host: null }),
        ],
        skipped: [{ file: 'bad.md', reason: 'not UTF-8 text' }],
        other_tips: 3,
      },
    });
    const host = await mount(<InboxDialog />);
    expect(host.textContent).toContain('Survey papers');
    expect(q(host, 'state question')?.textContent).toBe(STATE_BADGES.question.icon);
    expect(q(host, 'state queued')?.textContent).toBe('🕓');
    expect(host.textContent).toContain('session waiting for you');
    expect(host.textContent).toContain('new task');
    expect(host.textContent).toContain('bad.md');
    expect(host.textContent).toContain('3 other tip(s)');
    expect(q(host, 'open session window of new')?.disabled).toBe(true);
    await act(async () => q(host, 'open session window of survey')?.click());
    expect(openSessionWindow).toHaveBeenCalledWith('20261109T063100Z', 's1');
    await act(async () => q(host, 'open graph of survey')?.click());
    expect(openGraph).toHaveBeenCalledWith('C:/data/inbox/survey-1234abcd');
    expect(store.getState().open).toBe(false);
  });

  it('settings: suggestion, enable error, scan now disabled when off', async () => {
    const ipc = createMockIpc({ seed: false });
    const store = createInboxStore(ipc, createMockInboxApi());
    resetInboxStore(store);
    await store.getState().load();
    const host = await mount(<InboxPanel />);
    expect(host.textContent).toContain('Suggested:');
    expect(host.textContent).toContain('Not scanned yet.');
    const scan = [...host.querySelectorAll('button')].find((b) => b.textContent?.includes('Scan now'));
    expect(scan?.disabled).toBe(true);
    await act(async () => q<HTMLInputElement>(host, 'enable the inbox')?.click());
    expect(host.textContent).toMatch(/choose the tips folder/);
    const use = [...host.querySelectorAll('button')].find((b) => b.textContent === 'Use it');
    await act(async () => use?.click());
    expect(q(host, 'tips folder')?.textContent).toContain('Vindictive');
    await act(async () => q<HTMLInputElement>(host, 'start with Windows')?.click());
    expect(store.getState().app?.autostart).toBe(true);
  });

  it('button shows waiting questions and opens the dialog', async () => {
    const store = createInboxStore(createMockIpc({ seed: false }), createMockInboxApi());
    resetInboxStore(store);
    store.setState({ status: { ...emptyInboxStatus(), tips: [tip(), tip({ id: 'b' })] } });
    const host = await mount(<InboxButton />);
    expect(host.textContent).toContain('(2 ❓)');
    await act(async () => q(host, 'open the inbox')?.click());
    expect(store.getState().open).toBe(true);
    expect(when(null)).toBe('—');
    expect(when('garbage')).toBe('garbage');
  });
});

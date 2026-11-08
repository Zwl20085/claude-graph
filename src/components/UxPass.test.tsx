/** UX pass (v0.6): behaviour of the new pieces, item by item. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { StoreApi } from 'zustand/vanilla';
import type { RunState, TerminalSession } from '../types/contract';
import { api } from '../lib/api';
import { MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { Markdown } from '../lib/markdown';
import { STATUS_COLORS } from '../lib/status';
import { createGraphStore, installStore, type GraphStore } from '../store/graphStore';
import { closedSessionNotices, runEndNotice } from '../store/runWatch';
import { nextNodePosition, windowTitle } from '../App';
import { addStarterChain, EmptyCanvas, StatusLegend } from './CanvasOverlays';
import { Inspector } from './Inspector';
import { MessagesTab } from './MessagesTab';
import { HEADLESS_ONLY, PromptTab } from './PromptTab';
import { RunSummaryStrip } from './RunSummaryStrip';
import { BYPASS_CONFIRM, SettingsPopover } from './SettingsPopover';
import { CANCEL_CONFIRM, handleShortcut } from './Shortcuts';
import { clampWidth, PANEL_MAX, PANEL_MIN } from './SidePanelResizer';
import { Toasts } from './Toasts';
import { SessionsDropdown } from './TopBarParts';

vi.mock('../lib/api', async () => {
  const mock = await import('../lib/ipc.mock');
  return { api: mock.createMockIpc({ stepMs: 1, trustAll: true, exampleRunMode: 'as_file' }), MOCK_MODE: true, isTauri: () => false };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let store: StoreApi<GraphStore>;

async function render(el: JSX.Element): Promise<void> {
  const host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root?.render(el));
}
const q = <T extends Element = HTMLElement>(sel: string): T | null => document.querySelector<T>(sel);
const button = (text: string): HTMLButtonElement | undefined =>
  [...document.querySelectorAll('button')].find((b) => b.textContent?.includes(text));
const click = async (el: Element | null | undefined): Promise<void> => {
  if (!el) throw new Error('element not found');
  await act(async () => (el as HTMLElement).click());
};
const flush = (): Promise<void> => act(async () => new Promise((r) => setTimeout(r, 10)));

const session = (patch: Partial<TerminalSession>): TerminalSession => ({
  sid: 's1', title: 'Literature survey', node_ids: ['survey', 'design', 'report'], status: 'running', pid: 1, current_node: 'survey', ...patch,
});
const nodeRun = (status: RunState['nodes'][string]['status'], cost: number | null = null) => ({
  status, started_at: 't', finished_at: null, session_id: null, cost_usd: cost, num_turns: null, reason: null,
});
function terminalRun(status: RunState['status'], nodes: RunState['nodes'], sessions: TerminalSession[]): RunState {
  return { run_id: 'R1', graph_dir: MOCK_EXAMPLE_DIR, started_at: 't', finished_at: null, status, nodes, mode: 'terminal', sessions };
}

beforeEach(async () => {
  store = createGraphStore(api, { autosaveMs: 60_000 });
  installStore(store);
  expect(await store.getState().openGraph(MOCK_EXAMPLE_DIR)).toBe(true);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
  vi.restoreAllMocks();
});

describe('1. needs-you signals', () => {
  it('the window title counts the questions waiting', () => {
    expect(windowTitle('g', false, 0)).toBe('Claude Graph — g');
    expect(windowTitle('g', true, 2)).toBe('(2) Claude Graph — g*');
  });
});

describe('2. headless-only fields in terminal mode', () => {
  it('Permission is disabled and tagged; resources are read-only', async () => {
    const node = store.getState().graph?.nodes[0];
    if (!node) throw new Error('no node');
    await render(<PromptTab node={node} />);
    const selects = [...document.querySelectorAll<HTMLSelectElement>('select')];
    expect(selects.some((s) => s.disabled && [...s.options].some((o) => o.value === 'acceptEdits'))).toBe(true);
    expect(document.body.textContent).toContain(HEADLESS_ONLY);
    expect(q('[aria-label="resources (headless only)"]')).not.toBeNull();
    expect(q<HTMLTextAreaElement>('textarea[aria-label="Prompt"]')?.placeholder).toBe('What should this step do?');
  });
});

describe('3. empty canvas', () => {
  it('offers Add node and a starter chain of three connected nodes', async () => {
    const add = vi.fn();
    await render(<EmptyCanvas onAddNode={add} />);
    await click(button('Add node'));
    expect(add).toHaveBeenCalledTimes(1);
    const before = store.getState().graph?.nodes.length ?? 0;
    const ids = addStarterChain();
    expect(ids).toHaveLength(3);
    const g = store.getState().graph;
    expect(g?.nodes.length).toBe(before + 3);
    expect(g?.edges.filter((e) => ids.includes(e.from) && ids.includes(e.to))).toHaveLength(2);
    expect(nextNodePosition(g ?? null).y).toBeGreaterThan(0);
  });
});

describe('4. status legend and colours', () => {
  it('lists every status; waiting is violet, blocked orange, queued amber', async () => {
    expect([STATUS_COLORS.waiting, STATUS_COLORS.blocked, STATUS_COLORS.queued]).toEqual(['#a78bfa', '#f97316', '#f59e0b']);
    await render(<StatusLegend />);
    await click(q('button[aria-label="status legend"]'));
    const legend = q('[role="dialog"][aria-label="Status legend"]');
    expect(legend?.querySelectorAll('li')).toHaveLength(9);
    expect(legend?.textContent).toContain('needs you');
  });
});

describe('5. a session window closes', () => {
  it('one toast names the node; the node shows a banner with Run from here', async () => {
    const graph = store.getState().graph;
    const running = terminalRun('running', { survey: nodeRun('running') }, [session({})]);
    const gone = terminalRun('running', { survey: nodeRun('failed') }, [session({ status: 'failed', pid: null, alive: false })]);
    const notices = closedSessionNotices(running, gone, graph);
    expect(notices).toHaveLength(1);
    expect(notices[0]?.text).toContain('the chain stopped at Literature survey');
    expect(notices[0]?.nodeId).toBe('survey');
    await act(async () => store.getState().applyRunState(running));
    await act(async () => store.getState().applyRunState(gone));
    expect(store.getState().toasts.some((t) => t.text.includes('closed — the chain stopped at'))).toBe(true);
    await act(async () => store.getState().select('survey'));
    await render(<Inspector />);
    expect(q('[aria-label="session closed"]')?.textContent).toContain('Run from here');
  });
});

describe('6. layout', () => {
  it('the side panel stays between 320 and 480 px; sessions collapse into a dropdown', async () => {
    expect([clampWidth(100), clampWidth(400), clampWidth(900)]).toEqual([PANEL_MIN, 400, PANEL_MAX]);
    await render(
      <SessionsDropdown count={2}>
        <span>chip-a</span>
      </SessionsDropdown>,
    );
    expect(document.body.textContent).not.toContain('chip-a');
    await click(q('button[aria-label="2 sessions"]'));
    expect(document.body.textContent).toContain('chip-a');
  });
});

describe('7. run end', () => {
  it('one notice with counts and cost; the strip offers Rerun failed', async () => {
    const before = terminalRun('running', { survey: nodeRun('running') }, []);
    const after = { ...terminalRun('failed', { survey: nodeRun('succeeded', 0.25), design: nodeRun('failed', 0.17) }, []), status: 'failed' as const };
    const n = runEndNotice(before, after, store.getState().graph);
    expect(n?.text).toBe('Run failed: 1 succeeded, 1 failed — $0.4200');
    expect(n?.nodeId).toBe('design');
    expect(runEndNotice(after, after, null)).toBeNull();
    await act(async () => store.getState().applyRunState(after));
    await render(<RunSummaryStrip />);
    expect(q('[aria-label="run summary"]')?.textContent).toContain('1 failed');
    expect(button('Rerun failed')).toBeDefined();
  });
});

describe('8. results as markdown', () => {
  it('renders headings, lists and code as elements, never raw HTML', async () => {
    await render(<Markdown text={'# Done\n\n- **one**\n- `two`\n\n<img src=x onerror=alert(1)>\n\n```\ncode\n```'} />);
    expect(q('[role="heading"]')?.textContent).toBe('Done');
    expect(q('ul')?.querySelectorAll('li')).toHaveLength(2);
    expect(q('strong')?.textContent).toBe('one');
    expect(q('img')).toBeNull();
    expect(document.body.textContent).toContain('<img src=x onerror=alert(1)>');
    expect(q('pre')?.textContent).toBe('code');
  });
});

describe('9. Chat keeps what was sent', () => {
  it('lines sent to a node survive leaving the tab; line breaks get a note', async () => {
    vi.spyOn(api, 'ptyWrite').mockResolvedValue(undefined);
    await act(async () => store.getState().applyRunState(terminalRun('running', { survey: nodeRun('running') }, [session({})])));
    await render(<MessagesTab nodeId="survey" />);
    const box = q<HTMLTextAreaElement>('textarea[aria-label="message to this node"]') as HTMLTextAreaElement;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set?.call(box, 'one\ntwo');
      box.dispatchEvent(new Event('input', { bubbles: true }));
    });
    expect(document.body.textContent).toContain('the message goes as one line');
    expect(button('Send')?.title).toBe('Send (Ctrl+Enter)');
    await click(button('Send'));
    await act(async () => root?.unmount());
    root = null;
    await render(<MessagesTab nodeId="survey" />);
    expect(document.body.textContent).toContain('one two');
  });
});

describe('12. settings', () => {
  it('groups, headless-only tags, and a confirm before bypassPermissions', async () => {
    const confirm = vi.fn().mockReturnValue(false);
    await act(async () => store.getState().updateSettings({ run_mode: 'headless' }));
    await render(<SettingsPopover confirm={confirm} />);
    expect([...document.querySelectorAll('section[aria-label] > h3')].map((h) => h.textContent)).toEqual(['Run', 'Defaults', 'This machine']);
    const select = [...document.querySelectorAll<HTMLSelectElement>('select')].find((s) => [...s.options].some((o) => o.value === 'bypassPermissions'));
    await act(async () => {
      if (!select) throw new Error('no select');
      select.value = 'bypassPermissions';
      select.dispatchEvent(new Event('change', { bubbles: true }));
    });
    expect(confirm).toHaveBeenCalledWith(BYPASS_CONFIRM);
    expect(store.getState().graph?.settings.default_permission_mode).not.toBe('bypassPermissions');
    expect(document.body.textContent).toContain('Stop when a node fails');
    await act(async () => store.getState().updateSettings({ run_mode: 'terminal' }));
    expect(document.body.textContent).toContain(`Default model (${HEADLESS_ONLY})`);
  });
});

describe('13. inspector', () => {
  it('tabs are Prompt · Brief · Chat · Results, kept across nodes; the id sits under Advanced', async () => {
    await act(async () => store.getState().select('survey'));
    await render(<Inspector />);
    expect([...document.querySelectorAll('[role="tab"]')].map((t) => t.textContent)).toEqual(['Prompt', 'Brief', 'Chat', 'Results']);
    await click(button('Results'));
    await act(async () => store.getState().select('design'));
    expect(q('[role="tab"][aria-selected="true"]')?.textContent).toBe('Results');
    expect(q('details')?.textContent).toContain('Advanced');
    expect(q('details input[aria-label="Node id"]')).not.toBeNull();
    await flush();
  });
});

describe('14. shortcuts', () => {
  const key = (k: string, ctrl = false): KeyboardEvent => new KeyboardEvent('keydown', { key: k, ctrlKey: ctrl });
  it('Ctrl+N, Ctrl+J, ? and Esc (with a confirm) reach their actions', async () => {
    const h = { addNode: vi.fn(), toggleLog: vi.fn(), showSheet: vi.fn(), confirm: vi.fn().mockReturnValue(true) };
    expect(handleShortcut(key('n', true), h)).toBe(true);
    expect(handleShortcut(key('j', true), h)).toBe(true);
    expect(handleShortcut(key('?'), h)).toBe(true);
    expect([h.addNode, h.toggleLog, h.showSheet].map((f) => f.mock.calls.length)).toEqual([1, 1, 1]);
    expect(handleShortcut(key('Escape'), h)).toBe(false);
    const cancel = vi.spyOn(store.getState(), 'cancelRun');
    await act(async () => store.getState().applyRunState(terminalRun('running', { survey: nodeRun('running') }, [session({})])));
    expect(handleShortcut(key('Escape'), h)).toBe(true);
    expect(h.confirm).toHaveBeenCalledWith(CANCEL_CONFIRM);
    void cancel;
  });
});

describe('15. errors', () => {
  it('a toast action (Retry) runs once and the toast closes; Show reveals the node', async () => {
    const retry = vi.fn();
    await render(<Toasts />);
    await act(async () => store.getState().pushToast('error', 'Message to survey not sent', 'survey', { label: 'Retry', run: retry }));
    await click(button('Retry'));
    expect(retry).toHaveBeenCalledTimes(1);
    expect(store.getState().toasts).toEqual([]);
    const history = store.getState().toastHistory;
    expect(history[history.length - 1]?.text).toBe('Message to survey not sent');
    await act(async () => store.getState().pushToast('warning', 'Look', 'design'));
    await click(button('Show'));
    expect(store.getState().selectedNodeId).toBe('design');
  });
});

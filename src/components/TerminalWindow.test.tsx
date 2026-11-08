/** Session window: hash routing to it, and its wiring to the pty commands (xterm and the IPC mocked). */
import { act } from 'react';
import { createRoot, type Root as ReactRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { PtyData, RunState, TerminalSession } from '../types/contract';

const h = vi.hoisted(() => {
  class FakeTerm {
    static all: FakeTerm[] = [];
    writes: string[] = [];
    cols = 100;
    rows = 30;
    opened: Element | null = null;
    disposed = false;
    dataCb: ((d: string) => void) | null = null;
    resizeCb: ((s: { cols: number; rows: number }) => void) | null = null;
    constructor(public options: unknown) {
      FakeTerm.all.push(this);
    }
    loadAddon(): void {}
    open(el: Element): void {
      this.opened = el;
    }
    write(d: string): void {
      this.writes.push(d);
    }
    onData(cb: (d: string) => void) {
      this.dataCb = cb;
      return { dispose: () => (this.dataCb = null) };
    }
    onResize(cb: (s: { cols: number; rows: number }) => void) {
      this.resizeCb = cb;
      return { dispose: () => (this.resizeCb = null) };
    }
    focus(): void {}
    dispose(): void {
      this.disposed = true;
    }
  }
  let replayResolve: (r: { text: string; seq: number }) => void = () => undefined;
  const handlers: { pty: ((d: PtyData) => void) | null; state: ((s: RunState) => void) | null } = { pty: null, state: null };
  const api = {
    listSessions: vi.fn(),
    ptyReplay: vi.fn(() => new Promise<{ text: string; seq: number }>((r) => (replayResolve = r))),
    ptyWrite: vi.fn(async () => undefined),
    ptyResize: vi.fn(async () => undefined),
    listHarnesses: vi.fn(async (): Promise<unknown[]> => []),
    onPtyData: vi.fn(async (fn: (d: PtyData) => void) => {
      handlers.pty = fn;
      return () => (handlers.pty = null);
    }),
    onRunState: vi.fn(async (fn: (s: RunState) => void) => {
      handlers.state = fn;
      return () => (handlers.state = null);
    }),
  };
  return { FakeTerm, api, handlers, replay: (text: string, seq = 0) => replayResolve({ text, seq }) };
});

vi.mock('@xterm/xterm', () => ({ Terminal: h.FakeTerm }));
vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit(): void {}
  },
}));
vi.mock('../lib/api', () => ({ api: h.api, isTauri: () => false, MOCK_MODE: true }));
vi.mock('../MainApp', () => ({ default: () => <div>main window</div> }));

import TerminalWindow from './TerminalWindow';
import { Root } from '../Root';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const session = (patch: Partial<TerminalSession> = {}): TerminalSession => ({
  sid: 's1',
  title: 'Survey + Notes',
  node_ids: ['survey', 'design'],
  status: 'running',
  pid: 42,
  current_node: 'survey',
  ...patch,
});

let root: ReactRoot | null = null;
const flush = () => act(async () => new Promise((r) => setTimeout(r, 0)));

async function render(el: JSX.Element): Promise<void> {
  const host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root?.render(el));
  for (let i = 0; i < 5; i += 1) await flush();
}

beforeEach(() => {
  h.FakeTerm.all = [];
  h.api.listSessions.mockResolvedValue([session()]);
  Object.values(h.api).forEach((f) => f.mockClear());
});
afterEach(async () => {
  if (root) await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
  window.location.hash = '';
});

describe('hash routing', () => {
  it('#/terminal/<run>/<sid> renders the session window, no hash renders the main app', async () => {
    window.location.hash = '#/terminal/R1/s1';
    await render(<Root />);
    expect(document.querySelector('[aria-label="terminal of session s1"]')).not.toBeNull();
    expect(document.body.textContent).not.toContain('main window');
    await act(async () => root?.unmount());

    window.location.hash = '';
    await render(<Root />);
    expect(document.body.textContent).toContain('main window');
    expect(h.FakeTerm.all).toHaveLength(1);
  });
});

describe('TerminalWindow', () => {
  it('replays first, then writes live pty:data of its own session newer than the replay', async () => {
    await render(<TerminalWindow runId="R1" sid="s1" dir="/g" />);
    const term = h.FakeTerm.all[0];
    expect(term?.opened).not.toBeNull();
    expect(h.api.ptyReplay).toHaveBeenCalledWith('R1', 's1');
    // Arrive while the replay is still loading: seq 2 is in the replay (dropped), seq 3 is written after it.
    await act(async () => {
      h.handlers.pty?.({ run_id: 'R1', sid: 's1', data: 'in-replay', seq: 2 });
      h.handlers.pty?.({ run_id: 'R1', sid: 's1', data: 'live-3', seq: 3 });
    });
    expect(term?.writes).toEqual([]);
    await act(async () => h.replay('REPLAY', 2));
    await flush();
    expect(term?.writes).toEqual(['REPLAY', 'live-3']);
    await act(async () => {
      h.handlers.pty?.({ run_id: 'R1', sid: 's2', data: 'other session', seq: 9 });
      h.handlers.pty?.({ run_id: 'R2', sid: 's1', data: 'other run', seq: 9 });
      h.handlers.pty?.({ run_id: 'R1', sid: 's1', data: 'old', seq: 1 });
      h.handlers.pty?.({ run_id: 'R1', sid: 's1', data: 'live-4', seq: 4 });
    });
    expect(term?.writes).toEqual(['REPLAY', 'live-3', 'live-4']);
  });

  it('keystrokes go to pty_write, size changes to pty_resize (initial size too)', async () => {
    await render(<TerminalWindow runId="R1" sid="s1" dir={null} />);
    await act(async () => h.replay(''));
    await flush();
    const term = h.FakeTerm.all[0];
    expect(h.api.ptyResize).toHaveBeenCalledWith('R1', 's1', 100, 30);
    await act(async () => term?.dataCb?.('y\r'));
    expect(h.api.ptyWrite).toHaveBeenCalledWith('R1', 's1', 'y\r');
    await act(async () => term?.resizeCb?.({ cols: 80, rows: 24 }));
    expect(h.api.ptyResize).toHaveBeenLastCalledWith('R1', 's1', 80, 24);
  });

  it('header shows the title, status and nodes with the current one highlighted; run:state updates it', async () => {
    await render(<TerminalWindow runId="R1" sid="s1" dir="/g" />);
    expect(h.api.listSessions).toHaveBeenCalledWith('/g', 'R1');
    expect(document.body.textContent).toContain('Survey + Notes');
    expect(document.title).toBe('Claude Code — Survey + Notes');
    expect(document.querySelector('[aria-current="step"]')?.textContent).toBe('survey');
    const rs: RunState = {
      run_id: 'R1', graph_dir: '/g', started_at: 't', finished_at: null, status: 'running', nodes: {},
      mode: 'terminal', sessions: [session({ current_node: 'design' })],
    };
    await act(async () => h.handlers.state?.(rs));
    expect(document.querySelector('[aria-current="step"]')?.textContent).toBe('design');
    await act(async () => h.handlers.state?.({ ...rs, sessions: [session({ status: 'done', current_node: null })] }));
    expect(document.querySelector('[aria-label="session status"]')?.textContent).toBe('done');
    expect(document.querySelector('[aria-current="step"]')).toBeNull();
  });

  it('header and window title show the session harness label from list_harnesses', async () => {
    h.api.listHarnesses.mockResolvedValue([{ id: 'my-cli', label: 'My CLI', kind: 'custom' }]);
    h.api.listSessions.mockResolvedValue([session({ harness: 'my-cli' })]);
    await render(<TerminalWindow runId="R1" sid="s1" dir="/g" />);
    expect(h.api.listHarnesses).toHaveBeenCalledTimes(1);
    expect(document.querySelector('[aria-label="harness"]')?.textContent).toBe('My CLI');
    expect(document.title).toBe('My CLI — Survey + Notes');
  });

  it('a claude session (or one from an older run) shows Claude Code', async () => {
    await render(<TerminalWindow runId="R1" sid="s1" dir="/g" />);
    expect(document.querySelector('[aria-label="harness"]')?.textContent).toBe('Claude Code');
  });

  it('errors show in an inline banner that can be dismissed; unmount disposes the terminal', async () => {
    h.api.listSessions.mockRejectedValue({ code: 'not_found', message: 'run R1 not found' });
    h.api.ptyWrite.mockRejectedValueOnce({ code: 'not_found', message: 'session gone' });
    await render(<TerminalWindow runId="R1" sid="s1" dir={null} />);
    expect(document.querySelector('[role="alert"]')?.textContent).toContain('run R1 not found');
    await act(async () => h.replay(''));
    await flush();
    await act(async () => h.FakeTerm.all[0]?.dataCb?.('x'));
    await flush();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain('session gone');
    await act(async () => (document.querySelector('[aria-label="dismiss error"]') as HTMLButtonElement).click());
    expect(document.querySelector('[role="alert"]')).toBeNull();
    await act(async () => root?.unmount());
    root = null;
    expect(h.FakeTerm.all[0]?.disposed).toBe(true);
    expect(h.handlers.pty).toBeNull();
  });
});

/**
 * A Claude Code session window (`#/terminal/<run>/<sid>`): xterm.js on the
 * session's pty. On mount the transcript so far is replayed, then live
 * `pty:data` chunks newer than the replay follow; keystrokes go to `pty_write`, size changes to
 * `pty_resize`. No graph store, no canvas. Closing the window leaves the pty
 * alive (it can be reopened from the main window).
 */
import { useEffect, useRef, useState, type RefObject } from 'react';
import '@xterm/xterm/css/xterm.css';
import type { ITerminalOptions } from '@xterm/xterm';
import {
  isRunState,
  sessionHarness,
  sessionsOf,
  type HarnessProfile,
  type PtyData,
  type TerminalSession,
} from '../types/contract';
import { api, isTauri } from '../lib/api';
import { BUILTIN_HARNESSES, harnessLabel } from '../lib/harness';
import { SESSION_COLORS } from '../lib/sessions';

export interface TerminalWindowProps {
  runId: string;
  sid: string;
  /** Graph folder for list_sessions, when the window URL carries it. */
  dir: string | null;
}

export const TERMINAL_OPTIONS: ITerminalOptions = {
  cursorBlink: true,
  fontFamily: 'Consolas, "Cascadia Mono", "Courier New", monospace',
  fontSize: 13,
  scrollback: 10_000,
  theme: {
    background: '#1e1e1e',
    foreground: '#d4d4d4',
    cursor: '#d4d4d4',
    selectionBackground: '#264f78',
    black: '#000000',
    red: '#cd3131',
    green: '#0dbc79',
    yellow: '#e5e510',
    blue: '#2472c8',
    magenta: '#bc3fbc',
    cyan: '#11a8cd',
    white: '#e5e5e5',
    brightBlack: '#666666',
    brightRed: '#f14c4c',
    brightGreen: '#23d18b',
    brightYellow: '#f5f543',
    brightBlue: '#3b8eea',
    brightMagenta: '#d670d6',
    brightCyan: '#29b8db',
    brightWhite: '#e5e5e5',
  },
};

const errorText = (err: unknown): string =>
  typeof err === 'object' && err !== null && 'message' in err ? String((err as { message: unknown }).message) : String(err);

async function loadXterm() {
  const [{ Terminal }, { FitAddon }] = await Promise.all([import('@xterm/xterm'), import('@xterm/addon-fit')]);
  return { Terminal, FitAddon };
}

/** Collects disposers; anything registered after dispose runs at once. */
function createScope() {
  let disposed = false;
  let disposers: Array<() => void> = [];
  return {
    get disposed() {
      return disposed;
    },
    own(fn: () => void): void {
      if (disposed) fn();
      else disposers = [...disposers, fn];
    },
    dispose(): void {
      disposed = true;
      const all = disposers;
      disposers = [];
      all.reverse().forEach((fn) => {
        try {
          fn();
        } catch {
          // a disposer that throws must not stop the others
        }
      });
    },
  };
}

/** Mounts xterm in `host` and wires it to the session's pty. */
function useSessionTerminal(host: RefObject<HTMLDivElement>, runId: string, sid: string, report: (text: string) => void): void {
  useEffect(() => {
    const scope = createScope();
    const fail = (what: string) => (err: unknown) => {
      if (!scope.disposed) report(`${what}: ${errorText(err)}`);
    };
    const start = async (): Promise<void> => {
      const { Terminal, FitAddon } = await loadXterm();
      const el = host.current;
      if (scope.disposed || !el) return;
      const term = new Terminal(TERMINAL_OPTIONS);
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(el);
      scope.own(() => term.dispose());
      const refit = (): void => {
        try {
          fit.fit();
        } catch {
          // not laid out yet (zero size): the next resize fits
        }
      };
      refit();

      // Live chunks wait in `early` until the replay is written; chunks the
      // replay already holds (seq <= replay.seq) are dropped.
      let early: PtyData[] | null = [];
      let floor = 0;
      const writeChunk = (d: PtyData): void => {
        if (d.seq > floor) term.write(d.data);
      };
      const off = await api.onPtyData((d) => {
        if (d.run_id !== runId || d.sid !== sid) return;
        if (early === null) writeChunk(d);
        else early = [...early, d];
      });
      scope.own(off);
      if (scope.disposed) return;
      try {
        const replay = await api.ptyReplay(runId, sid);
        term.write(replay.text);
        floor = replay.seq;
      } catch (err) {
        fail('Could not load the transcript')(err);
      }
      const pendingChunks = early;
      early = null;
      pendingChunks.forEach(writeChunk);

      const typing = term.onData((data) => void api.ptyWrite(runId, sid, data).catch(fail('Typing did not reach Claude Code')));
      const sizing = term.onResize(({ cols, rows }) => void api.ptyResize(runId, sid, cols, rows).catch(fail('Resize failed')));
      scope.own(() => typing.dispose());
      scope.own(() => sizing.dispose());
      void api.ptyResize(runId, sid, term.cols, term.rows).catch(fail('Resize failed'));

      const RO = (globalThis as { ResizeObserver?: typeof ResizeObserver }).ResizeObserver;
      if (typeof RO === 'function') {
        const ro = new RO(() => refit());
        ro.observe(el);
        scope.own(() => ro.disconnect());
      }
      window.addEventListener('resize', refit);
      scope.own(() => window.removeEventListener('resize', refit));
      term.focus();
    };
    start().catch(fail('The terminal could not start'));
    return () => scope.dispose();
  }, [host, runId, sid, report]);
}

/** The session's title, status and node list: list_sessions, then live run:state. */
function useSessionInfo(runId: string, sid: string, dir: string | null, report: (text: string) => void): TerminalSession | null {
  const [session, setSession] = useState<TerminalSession | null>(null);
  useEffect(() => {
    let live = true;
    let fromEvent = false;
    api
      .listSessions(dir ?? '', runId)
      .then((list) => {
        if (live && !fromEvent) setSession(list.find((s) => s.sid === sid) ?? null);
      })
      .catch((err: unknown) => {
        if (live && !fromEvent) report(`Could not read the session: ${errorText(err)}`);
      });
    let off: (() => void) | null = null;
    api
      .onRunState((rs) => {
        if (!live || !isRunState(rs) || rs.run_id !== runId) return;
        const next = sessionsOf(rs).find((s) => s.sid === sid);
        if (!next) return;
        fromEvent = true;
        setSession(next);
      })
      .then((unlisten) => {
        if (live) off = unlisten;
        else unlisten();
      })
      .catch((err: unknown) => live && report(`Live session updates are missing: ${errorText(err)}`));
    return () => {
      live = false;
      off?.();
    };
  }, [runId, sid, dir, report]);
  return session;
}

/** Label of a harness id: built-ins at once, then this machine's list (list_harnesses). */
function useHarnessLabel(id: string): string {
  const [profiles, setProfiles] = useState<readonly HarnessProfile[]>(BUILTIN_HARNESSES);
  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => api.listHarnesses())
      .then((list) => live && setProfiles(list))
      .catch((err: unknown) => console.warn('list_harnesses failed; showing built-in labels', err));
    return () => {
      live = false;
    };
  }, []);
  return harnessLabel(profiles, id);
}

function useWindowTitle(title: string): void {
  useEffect(() => {
    document.title = title;
    if (!isTauri()) return;
    import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => getCurrentWindow().setTitle(title))
      .catch((err) => console.warn('setTitle failed', err));
  }, [title]);
}

function SessionHeader({ session, sid, harness }: { session: TerminalSession | null; sid: string; harness: string }) {
  const color = session ? SESSION_COLORS[session.status] : '#777777';
  return (
    <header className="flex shrink-0 flex-wrap items-center gap-2 border-b border-[#3c3c3c] bg-[#252526] px-3 py-1.5 text-[12px]">
      <span className="font-semibold text-[#d4d4d4]">{session?.title || `Session ${sid}`}</span>
      <span className="rounded border border-[#3c3c3c] px-1.5 py-0.5 text-[11px] text-[#9a9a9a]" aria-label="harness">
        {harness}
      </span>
      <span
        className="rounded-full px-2 py-0.5 text-[11px]"
        style={{ color, backgroundColor: `${color}26` }}
        aria-label="session status"
      >
        {session?.status ?? 'unknown'}
      </span>
      {session && (
        <ol className="flex flex-wrap items-center gap-1" aria-label="nodes of this session">
          {session.node_ids.map((id) => {
            const current = id === session.current_node;
            return (
              <li
                key={id}
                aria-current={current ? 'step' : undefined}
                className={`rounded px-1.5 py-0.5 font-mono text-[11px] ${current ? 'bg-[#3b82f6] text-white' : 'bg-[#1e1e1e] text-[#9a9a9a]'}`}
              >
                {id}
              </li>
            );
          })}
        </ol>
      )}
    </header>
  );
}

export default function TerminalWindow({ runId, sid, dir }: TerminalWindowProps) {
  const host = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const report = useRef((text: string) => setError(text)).current;
  const session = useSessionInfo(runId, sid, dir, report);
  useSessionTerminal(host, runId, sid, report);
  const harnessId = sessionHarness(session);
  const harness = useHarnessLabel(harnessId);
  useWindowTitle(`${harness} — ${session?.title || sid}`);

  return (
    <div className="flex h-screen flex-col bg-[#1e1e1e] text-[#d4d4d4]">
      <SessionHeader session={session} sid={sid} harness={harness} />
      {error && (
        <div role="alert" className="flex shrink-0 items-start gap-2 border-b border-[#ef4444] bg-[#ef4444]/10 px-3 py-1.5 text-[12px] text-[#ef4444]">
          <span className="flex-1">{error}</span>
          <button className="text-[#d4d4d4] hover:text-white" onClick={() => setError(null)} aria-label="dismiss error">
            ×
          </button>
        </div>
      )}
      <div ref={host} className="min-h-0 flex-1 p-1" role="region" aria-label={`terminal of session ${sid}`} />
    </div>
  );
}

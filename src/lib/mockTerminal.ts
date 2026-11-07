/**
 * Terminal-mode part of the mock backend: one fake pty per session whose
 * output is a canned transcript built from the mock node events, plus an
 * echoing keyboard. With `channel` set (browser dev), pty output and run
 * states are shared with other tabs, so `#/terminal/<run>/<sid>` opened in a
 * second tab shows the session of the first one.
 */
import type { AppError, Node, NodeEventKind, PtyData, PtyReplay, RunState, TerminalSession } from '../types/contract';
import { sessionsOf } from '../types/contract';
import type { Timers } from './debounce';
import type { MockRun } from './mockRunner';
import { sessionOfNode } from './sessions';

/** Delay of the fake reply to a line typed into a mock terminal. */
export const MOCK_PTY_REPLY_MS = 1000;
/** pty_replay returns at most this many characters (the backend caps at 512 KiB). */
export const REPLAY_CAP = 512 * 1024;
/** How long a tab waits for another tab to answer a replay request. */
export const REMOTE_REPLAY_MS = 400;
/** Same limit as the backend: longer or multi-line prompts are sent as a pointer to the brief. */
export const VERBATIM_PROMPT_MAX = 2000;

const ESC = '\x1b';
const color = (code: number, text: string): string => `${ESC}[${code}m${text}${ESC}[0m`;
const PROMPT = color(90, '> ');

/** The line terminal mode types for a node (TERMINAL_CONTRACT "The message"). */
export function terminalMessage(node: Node, graphDir: string): string {
  const prompt = node.prompt.trim();
  if (prompt !== '' && !prompt.includes('\n') && prompt.length <= VERBATIM_PROMPT_MAX) return prompt;
  const base = graphDir.replace(/[\\/]+$/, '');
  return `Node "${node.title || node.id}" (${node.id}): read the brief at ${base}/nodes/${node.id}/brief.md and do it; write all outputs to ${base}/nodes/${node.id}/deliverables.`;
}

function banner(dir: string, sid: string): string {
  return [
    color(33, '╭─ Claude Code (mock) ─────────────────────╮'),
    color(33, `│ session ${sid.padEnd(33)}│`),
    color(33, '╰──────────────────────────────────────────╯'),
    color(90, `  cwd: ${dir}`),
    '',
    '',
  ].join('\r\n');
}

function eventLine(kind: NodeEventKind, text: string): string | null {
  switch (kind) {
    case 'text':
      return `● ${text}\r\n`;
    case 'tool_use':
      return color(36, `⏺ ${text}`) + '\r\n';
    case 'tool_result':
      return color(90, `  ⎿ ${text}`) + '\r\n';
    case 'permission_denied':
      return color(33, `  ⚠ denied: ${text}`) + '\r\n';
    case 'stderr':
    case 'parse_error':
      return color(31, text) + '\r\n';
    case 'result':
      return color(32, `✓ ${text}`) + '\r\n\r\n' + PROMPT;
    default:
      return null;
  }
}

type BridgeMessage =
  | { t: 'pty'; d: PtyData }
  | { t: 'state'; s: RunState }
  | { t: 'write'; run_id: string; sid: string; text: string }
  | { t: 'replay?'; id: string; run_id: string; sid: string }
  | { t: 'replay!'; id: string; replay: PtyReplay };

/** The part of BroadcastChannel the mock uses (injectable in tests). */
export interface MockChannel {
  postMessage: (msg: unknown) => void;
  onmessage: ((ev: { data: unknown }) => void) | null;
}

export interface MockTerminalDeps {
  timers: Timers;
  replyMs: number;
  getRun: (runId: string) => MockRun | undefined;
  emitPty: (data: PtyData) => void;
  /** Run states received from another tab (delivered to this tab's run:state listeners). */
  emitRemoteState: (state: RunState) => void;
  openWindow?: (url: string, target: string) => void;
  channel?: MockChannel | null;
}

const appError = (code: string, message: string): AppError => ({ code, message });

export function createMockTerminal(deps: MockTerminalDeps) {
  /** Transcript (capped) and the seq of its last chunk, per session. */
  const transcripts = new Map<string, PtyReplay>();
  const lines = new Map<string, string>();
  const remoteStates = new Map<string, RunState>();
  const pending = new Map<string, (replay: PtyReplay | null) => void>();
  let requestSeq = 0;
  const key = (runId: string, sid: string): string => `${runId}/${sid}`;
  const post = (msg: BridgeMessage): void => deps.channel?.postMessage(msg);

  const localSession = (runId: string, sid: string): TerminalSession | null =>
    sessionsOf(deps.getRun(runId)?.state).find((s) => s.sid === sid) ?? null;
  /** `#/terminal/<run>/<sid>?dir=<graph dir>`, as the backend builds it. */
  const windowUrl = (runId: string, sid: string): string => {
    const dir = deps.getRun(runId)?.state.graph_dir ?? remoteStates.get(runId)?.graph_dir ?? '';
    const query = dir === '' ? '' : `?dir=${encodeURIComponent(dir)}`;
    return `#/terminal/${encodeURIComponent(runId)}/${encodeURIComponent(sid)}${query}`;
  };
  const remoteSession = (runId: string, sid: string): TerminalSession | null =>
    sessionsOf(remoteStates.get(runId)).find((s) => s.sid === sid) ?? null;

  const record = (k: string, data: string, seq: number): void => {
    const text = (transcripts.get(k)?.text ?? '') + data;
    transcripts.set(k, { text: text.length > REPLAY_CAP ? text.slice(text.length - REPLAY_CAP) : text, seq });
  };

  const out = (runId: string, sid: string, data: string): void => {
    const k = key(runId, sid);
    const first = !transcripts.has(k);
    const seq = (transcripts.get(k)?.seq ?? 0) + 1;
    record(k, data, seq);
    // Like the backend, the mock opens a session's window when the session starts.
    if (first) deps.openWindow?.(windowUrl(runId, sid), `term-${runId}-${sid}`);
    const chunk: PtyData = { run_id: runId, sid, data, seq };
    deps.emitPty(chunk);
    post({ t: 'pty', d: chunk });
  };

  /** Keyboard input of a local session: echo, line editing, fake reply on Enter. */
  const typeLocal = (runId: string, sid: string, text: string): void => {
    const k = key(runId, sid);
    let line = lines.get(k) ?? '';
    let echo = '';
    const plain = text.replace(new RegExp(`${ESC}\\[[0-9;?]*[A-Za-z~]`, 'g'), '').replace(new RegExp(`${ESC}O.`, 'g'), '');
    for (const ch of plain) {
      if (ch === '\r' || ch === '\n') {
        echo += '\r\n';
        const said = line.trim();
        line = '';
        if (said !== '') {
          deps.timers.setTimeout(() => out(runId, sid, `● (mock) Received: ${said}\r\n\r\n${PROMPT}`), deps.replyMs);
        }
      } else if (ch === '\x7f' || ch === '\b') {
        if (line.length > 0) {
          line = line.slice(0, -1);
          echo += '\b \b';
        }
      } else if (ch >= ' ') {
        line += ch;
        echo += ch;
      }
    }
    lines.set(k, line);
    if (echo !== '') out(runId, sid, echo);
  };

  const onBridge = (msg: BridgeMessage): void => {
    switch (msg.t) {
      case 'pty': {
        const k = key(msg.d.run_id, msg.d.sid);
        if (deps.getRun(msg.d.run_id)) return;
        record(k, msg.d.data, msg.d.seq);
        deps.emitPty(msg.d);
        return;
      }
      case 'state':
        if (deps.getRun(msg.s.run_id)) return;
        remoteStates.set(msg.s.run_id, msg.s);
        deps.emitRemoteState(msg.s);
        return;
      case 'write':
        if (localSession(msg.run_id, msg.sid)) typeLocal(msg.run_id, msg.sid, msg.text);
        return;
      case 'replay?':
        if (localSession(msg.run_id, msg.sid)) {
          post({ t: 'replay!', id: msg.id, replay: transcripts.get(key(msg.run_id, msg.sid)) ?? { text: '', seq: 0 } });
        }
        return;
      case 'replay!':
        pending.get(msg.id)?.(msg.replay);
        pending.delete(msg.id);
        return;
      default:
        return;
    }
  };

  if (deps.channel) {
    deps.channel.onmessage = (ev) => {
      const data = ev.data as BridgeMessage | null;
      if (data && typeof data === 'object' && typeof data.t === 'string') onBridge(data);
    };
  }

  const askRemote = (runId: string, sid: string): Promise<PtyReplay | null> =>
    new Promise((resolve) => {
      requestSeq += 1;
      const id = `${Date.now()}-${requestSeq}`;
      pending.set(id, resolve);
      post({ t: 'replay?', id, run_id: runId, sid });
      deps.timers.setTimeout(() => {
        if (pending.delete(id)) resolve(null);
      }, REMOTE_REPLAY_MS);
    });

  const requireKnown = (runId: string, sid: string): 'local' | 'remote' => {
    if (localSession(runId, sid)) return 'local';
    if (deps.channel) return 'remote';
    throw appError('not_found', `session ${sid} of run ${runId} not found`);
  };

  return {
    /** Feed one mock node event into the node's session transcript (terminal runs only). */
    onNodeEvent(runId: string, nodeId: string, kind: NodeEventKind, text: string): void {
      const run = deps.getRun(runId);
      if (!run || run.state.mode !== 'terminal') return;
      const session = sessionOfNode(sessionsOf(run.state), nodeId);
      if (!session) return;
      if (kind === 'system') {
        const node = run.graph.nodes.find((n) => n.id === nodeId);
        const first = !transcripts.has(key(runId, session.sid));
        const typed = node ? terminalMessage(node, run.state.graph_dir) : nodeId;
        out(runId, session.sid, `${first ? banner(run.state.graph_dir, session.sid) + PROMPT : ''}${typed}\r\n`);
        return;
      }
      const line = eventLine(kind, text);
      if (line !== null) out(runId, session.sid, line);
    },
    /** Share a run state with other tabs (browser dev only). */
    shareState(state: RunState): void {
      if (state.mode === 'terminal') post({ t: 'state', s: state });
    },
    list(dir: string, runId: string): TerminalSession[] {
      const run = deps.getRun(runId);
      if (run) {
        if (dir !== '' && run.state.graph_dir !== dir) throw appError('not_found', `run ${runId} not found in ${dir}`);
        return sessionsOf(run.state);
      }
      if (deps.channel) return sessionsOf(remoteStates.get(runId));
      throw appError('not_found', `run ${runId} not found`);
    },
    write(runId: string, sid: string, text: string): void {
      if (requireKnown(runId, sid) === 'local') typeLocal(runId, sid, text);
      else post({ t: 'write', run_id: runId, sid, text });
    },
    resize(runId: string, sid: string, cols: number, rows: number): void {
      if (!Number.isInteger(cols) || !Number.isInteger(rows) || cols < 1 || rows < 1) {
        throw appError('invalid_input', `bad terminal size ${cols}x${rows}`);
      }
      requireKnown(runId, sid);
    },
    async replay(runId: string, sid: string): Promise<PtyReplay> {
      if (requireKnown(runId, sid) === 'local') return { ...(transcripts.get(key(runId, sid)) ?? { text: '', seq: 0 }) };
      const remote = await askRemote(runId, sid);
      if (remote !== null) return remote;
      const known = transcripts.get(key(runId, sid));
      if (known !== undefined) return { ...known };
      const note = color(33, `(mock) No tab of this browser runs ${runId}. Start a run in the main tab, then open the session from its chip.`) + '\r\n';
      return { text: note, seq: 0 };
    },
    open(runId: string, sid: string): void {
      const session = localSession(runId, sid) ?? remoteSession(runId, sid);
      if (!session) throw appError('not_found', `session ${sid} of run ${runId} not found`);
      deps.openWindow?.(windowUrl(runId, sid), `term-${runId}-${sid}`);
    },
  };
}

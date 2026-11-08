/** Pure logic of the Messages tab: merging history with locally sent text, composer rules. */
import type { NodeMessage, NodeStatus } from '../types/contract';
import { assertNever } from './status';

export interface DisplayMessage extends NodeMessage {
  key: string;
  /** User message still waiting for an assistant reply. */
  queued: boolean;
}

/** Messages sent from this window that the backend has not listed yet. */
export interface LocalMessage {
  text: string;
  ts: string;
}

/**
 * Two records of one message may carry different timestamps: the backend saves
 * the line to messages.jsonl and then emits `user_message` with a fresh `ts`,
 * and a local send is stamped by this window before the backend sees it.
 * Records with the same role and text this close in time are the same message.
 */
export const SAME_MESSAGE_WINDOW_MS = 5000;

/** Milliseconds of an RFC 3339 timestamp (sub-millisecond digits dropped), or null. */
function tsMillis(ts: string): number | null {
  const n = Date.parse(ts.replace(/(\.\d{3})\d+/, '$1'));
  return Number.isNaN(n) ? null : n;
}

/** Equal timestamps, or both parse and lie within SAME_MESSAGE_WINDOW_MS. */
export function closeInTime(a: string, b: string): boolean {
  if (a === b) return true;
  const x = tsMillis(a);
  const y = tsMillis(b);
  return x !== null && y !== null && Math.abs(x - y) <= SAME_MESSAGE_WINDOW_MS;
}

/** A history entry stands for a local send: same text, saved at or after the send (or close to it). */
const coversLocal = (h: NodeMessage, l: LocalMessage): boolean =>
  h.role === 'user' && h.text === l.text && (closeInTime(h.ts, l.ts) || h.ts >= l.ts);

/** Local sends that no history entry stands for; each history entry covers at most one send. */
function unmatchedLocal(history: readonly NodeMessage[], local: readonly LocalMessage[]): LocalMessage[] {
  const used = new Set<number>();
  return local.filter((l) => {
    const i = history.findIndex((h, j) => !used.has(j) && coversLocal(h, l));
    if (i === -1) return true;
    used.add(i);
    return false;
  });
}

/**
 * History (from list_messages / live events) plus local sends not yet in it,
 * ordered by timestamp (stable). A user message is `queued` until an
 * assistant message follows it.
 */
export function displayMessages(history: readonly NodeMessage[], local: readonly LocalMessage[]): DisplayMessage[] {
  const pendingLocal = unmatchedLocal(history, local);
  const all: NodeMessage[] = [...history, ...pendingLocal.map((l) => ({ role: 'user' as const, text: l.text, ts: l.ts }))];
  const ordered = all
    .map((m, i) => ({ m, i }))
    .sort((a, b) => a.m.ts.localeCompare(b.m.ts) || a.i - b.i)
    .map(({ m }) => m);
  const lastAssistant = ordered.map((m) => m.role).lastIndexOf('assistant');
  return ordered.map((m, i) => ({ ...m, key: `${i}-${m.ts}`, queued: m.role === 'user' && i > lastAssistant }));
}

/** Drop local sends that the history now contains. Returns a new array. */
export function pruneLocal(history: readonly NodeMessage[], local: readonly LocalMessage[]): LocalMessage[] {
  return unmatchedLocal(history, local);
}

export interface ComposerState {
  enabled: boolean;
  hint: string | null;
}

/** Whether the user may message a node, given its status in the shown run (null = not in it). */
export function composerState(status: NodeStatus | null): ComposerState {
  if (status === null) return { enabled: false, hint: 'Run the node first.' };
  switch (status) {
    case 'running':
      return { enabled: true, hint: 'Delivered after the current turn finishes.' };
    case 'waiting':
      return { enabled: true, hint: 'The node asked you something; your message is its answer.' };
    case 'blocked':
      return { enabled: true, hint: 'The node asked you something. Reply here; a DONE reply marks it succeeded.' };
    case 'succeeded':
    case 'failed':
    case 'cancelled':
      return { enabled: true, hint: 'Sending resumes this node’s session.' };
    case 'pending':
    case 'queued':
      return { enabled: false, hint: 'Waiting for the node to start.' };
    case 'skipped':
      return { enabled: false, hint: 'Run the node first.' };
    default:
      return assertNever(status);
  }
}

/**
 * Snapshot (list_messages) plus live entries (user_message events), without
 * duplicates. A live entry is dropped when an unclaimed snapshot entry has the
 * same role and text and an equal or close timestamp (see closeInTime), or,
 * for a live entry not newer than the snapshot, any timestamp; each
 * snapshot entry absorbs at most one live entry, so a message really sent
 * twice stays twice. Live entries the snapshot does not hold are kept, so a
 * snapshot taken before an event never loses it (M5).
 */
export function mergeMessages(snapshot: readonly NodeMessage[], live: readonly NodeMessage[]): NodeMessage[] {
  const claimed = new Set<number>();
  const newest = snapshot.reduce((max, m) => (m.ts > max ? m.ts : max), '');
  const extra: NodeMessage[] = [];
  const find = (m: NodeMessage, when: (s: NodeMessage) => boolean): number =>
    snapshot.findIndex((s, j) => !claimed.has(j) && s.role === m.role && s.text === m.text && when(s));
  for (const m of live) {
    // Close in time, else (older than the snapshot's newest entry) the same text anywhere in it.
    const close = find(m, (s) => closeInTime(s.ts, m.ts));
    const i = close !== -1 || m.ts > newest ? close : find(m, () => true);
    if (i !== -1) {
      claimed.add(i);
      continue;
    }
    // The same event can be delivered twice (log replay); keep one copy.
    if (extra.some((e) => e.role === m.role && e.text === m.text && e.ts === m.ts)) continue;
    extra.push(m);
  }
  return [...snapshot, ...extra]
    .map((m, i) => ({ m, i }))
    .sort((a, b) => a.m.ts.localeCompare(b.m.ts) || a.i - b.i)
    .map(({ m }) => m);
}

export interface MessageHistory {
  /** Re-read the snapshot; live entries are kept and merged in. */
  refresh: () => Promise<void>;
  addLive: (messages: readonly NodeMessage[]) => void;
  current: () => NodeMessage[];
  dispose: () => void;
}

/**
 * History of one node session: the latest snapshot merged with live entries.
 * A snapshot that resolves after newer live entries arrived never drops them
 * (M5); an older refresh that resolves after a newer one is ignored.
 */
export function createMessageHistory(
  list: () => Promise<NodeMessage[]>,
  onChange: (messages: NodeMessage[]) => void,
  onError: (err: unknown) => void = () => undefined,
): MessageHistory {
  let snapshot: NodeMessage[] = [];
  let live: NodeMessage[] = [];
  let request = 0;
  let disposed = false;
  const emit = (): void => {
    if (!disposed) onChange(mergeMessages(snapshot, live));
  };
  return {
    refresh: async () => {
      request += 1;
      const mine = request;
      try {
        const next = await list();
        if (disposed || mine !== request) return;
        snapshot = next;
        emit();
      } catch (err) {
        if (!disposed) onError(err);
      }
    },
    addLive: (messages) => {
      if (messages.length === 0) return;
      live = [...live, ...messages];
      emit();
    },
    current: () => mergeMessages(snapshot, live),
    dispose: () => {
      disposed = true;
    },
  };
}

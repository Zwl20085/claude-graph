/** Pure logic of the Messages tab: merging history with locally sent text, composer rules. */
import type { NodeMessage, NodeStatus } from '../types/contract';

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
 * History (from list_messages / live events) plus local sends not yet in it,
 * ordered by timestamp (stable). A user message is `queued` until an
 * assistant message follows it.
 */
export function displayMessages(history: readonly NodeMessage[], local: readonly LocalMessage[]): DisplayMessage[] {
  const unmatched = history.filter((m) => m.role === 'user').map((m) => m.text);
  const pendingLocal = local.filter((l) => {
    const i = unmatched.indexOf(l.text);
    if (i === -1) return true;
    unmatched.splice(i, 1);
    return false;
  });
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
  return local.filter((l) => !history.some((h) => h.role === 'user' && h.text === l.text && h.ts >= l.ts));
}

export interface ComposerState {
  enabled: boolean;
  hint: string | null;
}

/** Whether the user may message a node, given its status in the shown run (null = not in it). */
export function composerState(status: NodeStatus | null): ComposerState {
  switch (status) {
    case 'running':
      return { enabled: true, hint: 'Delivered after the current turn finishes.' };
    case 'succeeded':
    case 'failed':
    case 'cancelled':
      return { enabled: true, hint: 'Sending resumes this node’s session.' };
    case 'pending':
    case 'queued':
      return { enabled: false, hint: 'Waiting for the node to start.' };
    default:
      return { enabled: false, hint: 'Run the node first.' };
  }
}

const messageKey = (m: NodeMessage): string => `${m.role}\u0000${m.ts}\u0000${m.text}`;

/**
 * Snapshot (list_messages) plus live entries (user_message events), without
 * duplicates on (role, ts, text). A live entry is never dropped when it is
 * newer than the snapshot; an older live entry is dropped only when the
 * snapshot already holds the same role and text (same message, other ts).
 */
export function mergeMessages(snapshot: readonly NodeMessage[], live: readonly NodeMessage[]): NodeMessage[] {
  const seen = new Set(snapshot.map(messageKey));
  const newest = snapshot.reduce((max, m) => (m.ts > max ? m.ts : max), '');
  const extra: NodeMessage[] = [];
  for (const m of live) {
    const key = messageKey(m);
    if (seen.has(key)) continue;
    const covered = m.ts <= newest && snapshot.some((s) => s.role === m.role && s.text === m.text);
    if (covered) continue;
    seen.add(key);
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

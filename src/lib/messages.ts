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

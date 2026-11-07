/** Pure helpers for run/log state in the graph store. */
import type { NodeEvent, NodeEventKind, RunState, RunSummary } from '../types/contract';
import { DENIED_HINT } from '../lib/status';

export const LOG_CAP = 2000;

export interface LogLine {
  seq: number;
  kind: NodeEventKind;
  text: string;
  ts: string;
}

export type EventsByNode = Readonly<Record<string, readonly LogLine[]>>;

/** Append one line, dropping the oldest beyond `cap`. Returns a new array. */
export function appendCapped<T>(lines: readonly T[], line: T, cap: number = LOG_CAP): T[] {
  const next = [...lines, line];
  return next.length > cap ? next.slice(next.length - cap) : next;
}

/** Add a node:event to the per-node buffers; returns a new map. */
export function addEvent(events: EventsByNode, ev: NodeEvent, seq: number, cap: number = LOG_CAP): EventsByNode {
  const line: LogLine = { seq, kind: ev.kind, text: ev.text, ts: ev.ts };
  return { ...events, [ev.node_id]: appendCapped(events[ev.node_id] ?? [], line, cap) };
}

/** Compare folder paths loosely (slash style, trailing slash, case on Windows-like paths). */
export function samePath(a: string | null, b: string | null): boolean {
  if (a === null || b === null) return false;
  const norm = (p: string): string => {
    const s = p.replace(/\\/g, '/').replace(/\/+$/, '').replace(/^\/\/\?\//, '');
    return /^[a-zA-Z]:/.test(s) ? s.toLowerCase() : s;
  };
  return norm(a) === norm(b);
}

/**
 * Whether an incoming run:state belongs to the open graph: the shown run, or
 * any run of the open folder. Runs whose folder spelling differs (the backend
 * canonicalises paths) are buffered by the store while a start is pending.
 */
export function acceptsRunState(rs: RunState, dir: string | null, current: RunState | null): boolean {
  return rs.run_id === current?.run_id || samePath(rs.graph_dir, dir);
}

const TERMINAL = new Set(['succeeded', 'failed', 'blocked', 'skipped', 'cancelled']);

/** Number of nodes in a terminal status, plus one when the run itself has finished. */
export function progressOf(rs: RunState): number {
  const done = Object.values(rs.nodes).filter((n) => TERMINAL.has(n.status)).length;
  return done + (rs.status === 'running' ? 0 : 1);
}

/**
 * Which state to keep when a fetched RunState (get_run reply) lands: the
 * fetched one only if nothing of that run is shown yet, or if it is strictly
 * further along. Live run:state events never go through this.
 */
export function preferFetched(current: RunState | null, fetched: RunState): RunState {
  if (current === null || current.run_id !== fetched.run_id) return fetched;
  return progressOf(fetched) > progressOf(current) ? fetched : current;
}

/** Newest run first (run ids are UTC timestamps, started_at as tie-break). */
export function newestFirst(runs: readonly RunSummary[]): RunSummary[] {
  return [...runs].sort((a, b) => b.started_at.localeCompare(a.started_at) || b.run_id.localeCompare(a.run_id));
}

/** Denied tools seen live per node of the shown run (permission_denied events). */
export type DenialsByNode = Readonly<Record<string, readonly string[]>>;

/** Tool name of a permission_denied event text (`WebSearch: "query…"` -> `WebSearch`). */
export function deniedToolName(text: string): string {
  const name = text.split(':')[0]?.trim() ?? '';
  return name === '' ? 'a tool' : name;
}

/** Record one denial; returns the new map and whether it was the node's first in this run. */
export function addDenial(denials: DenialsByNode, nodeId: string, tool: string): { denials: DenialsByNode; first: boolean } {
  const seen = denials[nodeId] ?? [];
  const first = seen.length === 0;
  if (seen.includes(tool)) return { denials, first };
  return { denials: { ...denials, [nodeId]: [...seen, tool] }, first };
}

export function deniedToast(nodeId: string, tool: string): string {
  return `Node ${nodeId}: ${tool} was denied. ${DENIED_HINT}`;
}

/**
 * Whether a node id may still change (by hand or by following its title):
 * created this session and not run, or absent from every loaded run.
 * `ranNodeIds` null = the run list is not loaded yet (only fresh nodes qualify).
 */
export function idIsEditable(
  s: { freshNodeIds: readonly string[]; ranNodeIds: readonly string[] | null; runState: RunState | null },
  id: string,
): boolean {
  if (s.freshNodeIds.includes(id)) return true;
  if (s.ranNodeIds === null || s.ranNodeIds.includes(id)) return false;
  return !(s.runState !== null && id in s.runState.nodes);
}

/** Union of node ids, keeping order, without duplicates. */
export function unionIds(a: readonly string[], b: Iterable<string>): string[] {
  const out = [...a];
  for (const id of b) if (!out.includes(id)) out.push(id);
  return out;
}

export function errorMessage(err: unknown): string {
  if (typeof err === 'object' && err !== null && 'message' in err) return String((err as { message: unknown }).message);
  return String(err);
}

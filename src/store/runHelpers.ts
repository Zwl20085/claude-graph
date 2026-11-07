/** Pure helpers for run/log state in the graph store. */
import type { NodeEvent, NodeEventKind, RunState, RunSummary } from '../types/contract';

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

const TERMINAL = new Set(['succeeded', 'failed', 'skipped', 'cancelled']);

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

export function errorMessage(err: unknown): string {
  if (typeof err === 'object' && err !== null && 'message' in err) return String((err as { message: unknown }).message);
  return String(err);
}

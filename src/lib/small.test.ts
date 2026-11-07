import { describe, expect, it, vi } from 'vitest';
import type { RunState } from '../types/contract';
import { createDebouncer } from './debounce';
import { isValidSlug, slugify, uniqueSlug } from './slug';
import * as st from './status';
import { isTauri, selectApi } from './api';

describe('slug', () => {
  it.each([
    ['Literature survey', 'literature-survey'],
    ['  --Hello__World!! ', 'hello-world'],
    ['Café déjà vu', 'cafe-deja-vu'],
    ['', 'node'],
    ['日本語', 'node'],
    ['CON', 'con-node'],
    ['a'.repeat(60), 'a'.repeat(40)],
  ])('slugify(%j) = %j', (input, out) => {
    expect(slugify(input)).toBe(out);
    expect(isValidSlug(out)).toBe(true);
  });

  it('uniqueSlug appends -2, -3', () => {
    expect(uniqueSlug('New node', [])).toBe('new-node');
    expect(uniqueSlug('New node', ['new-node', 'new-node-2'])).toBe('new-node-3');
  });

  it('isValidSlug rejects bad ids', () => {
    expect(isValidSlug('a--b')).toBe(false);
    expect(isValidSlug('-a')).toBe(false);
    expect(isValidSlug('A')).toBe(false);
    expect(isValidSlug('nul')).toBe(false);
  });
});

describe('debounce (autosave timing)', () => {
  it('fires once, ms after the last schedule', () => {
    vi.useFakeTimers();
    const fn = vi.fn();
    const d = createDebouncer(fn, 800);
    d.schedule();
    vi.advanceTimersByTime(500);
    d.schedule();
    vi.advanceTimersByTime(799);
    expect(fn).not.toHaveBeenCalled();
    expect(d.pending()).toBe(true);
    vi.advanceTimersByTime(1);
    expect(fn).toHaveBeenCalledTimes(1);
    expect(d.pending()).toBe(false);
    vi.useRealTimers();
  });

  it('flush runs a pending call immediately; cancel drops it', () => {
    vi.useFakeTimers();
    const fn = vi.fn();
    const d = createDebouncer(fn, 800);
    expect(d.flush()).toBe(false);
    d.schedule();
    expect(d.flush()).toBe(true);
    expect(fn).toHaveBeenCalledTimes(1);
    d.schedule();
    d.cancel();
    vi.advanceTimersByTime(2000);
    expect(fn).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });
});

const runWith = (status: RunState['status'], nodeStatuses: RunState['nodes'][string]['status'][]): RunState => ({
  run_id: 'r',
  graph_dir: '/g',
  started_at: 'x',
  finished_at: null,
  status,
  nodes: Object.fromEntries(
    nodeStatuses.map((s, i) => [
      `n${i}`,
      { status: s, started_at: null, finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: null },
    ]),
  ),
});

describe('status -> style', () => {
  it('ring colours per status', () => {
    expect(st.ringStyle(null)).toEqual({ borderColor: st.IDLE_BORDER, borderStyle: 'solid', className: '' });
    expect(st.ringStyle('pending').borderColor).toBe('#555555');
    expect(st.ringStyle('queued').borderColor).toBe('#f59e0b');
    expect(st.ringStyle('running')).toEqual({ borderColor: '#3b82f6', borderStyle: 'solid', className: 'cg-pulse' });
    expect(st.ringStyle('succeeded').borderColor).toBe('#22c55e');
    expect(st.ringStyle('failed').borderColor).toBe('#ef4444');
    expect(st.ringStyle('skipped')).toMatchObject({ borderColor: '#777777', borderStyle: 'dashed' });
    expect(st.ringStyle('cancelled').borderColor).toBe('#b45309');
  });

  it('edge look follows the source status', () => {
    expect(st.edgeVisual('running')).toEqual({ stroke: '#3b82f6', animated: true, glow: '#3b82f6' });
    expect(st.edgeVisual('succeeded').stroke).toBe('#22c55e');
    expect(st.edgeVisual('failed').stroke).toBe('#ef4444');
    expect(st.edgeVisual(null)).toEqual({ stroke: '#8a8a8a', animated: false, glow: null });
    expect(st.edgeVisual('queued').stroke).toBe('#8a8a8a');
  });

  it('model colours', () => {
    expect(st.MODEL_COLORS).toEqual({ opus: '#8b5cf6', sonnet: '#3b82f6', haiku: '#22c55e', inherit: '#6b7280' });
  });

  it('counts, active, failures, cost', () => {
    const r = runWith('failed', ['failed', 'skipped', 'succeeded']);
    expect(st.countStatuses(r)).toMatchObject({ failed: 1, skipped: 1, succeeded: 1, running: 0 });
    expect(st.runHasFailures(r)).toBe(true);
    expect(st.runHasFailures(runWith('running', ['failed']))).toBe(false);
    expect(st.runHasFailures(runWith('succeeded', ['succeeded']))).toBe(false);
    expect(st.runHasFailures(null)).toBe(false);
    expect(st.isRunActive(runWith('running', []))).toBe(true);
    expect(st.formatCost(null)).toBe('—');
    expect(st.formatCost(0.01234)).toBe('$0.0123');
    expect(st.formatCost(2.5)).toBe('$2.50');
  });
});

describe('api selection', () => {
  it('uses the mock without __TAURI_INTERNALS__', () => {
    expect(isTauri({})).toBe(false);
    expect(isTauri(null)).toBe(false);
    expect(selectApi({}).mock).toBe(true);
  });
  it('uses the real IPC inside Tauri', () => {
    expect(isTauri({ __TAURI_INTERNALS__: {} })).toBe(true);
    expect(selectApi({ __TAURI_INTERNALS__: {} }).mock).toBe(false);
  });
});

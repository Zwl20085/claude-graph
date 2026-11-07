import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Api } from '../lib/api';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import type { LaunchOptions } from '../types/contract';
import { createGraphStore } from './graphStore';

type LaunchWindow = { __CG_LAUNCH__?: Partial<LaunchOptions> };
const win = globalThis as LaunchWindow;
const lastToast = <T>(list: readonly T[]): T | undefined => list[list.length - 1];

function setup(overrides: Partial<Api> = {}) {
  const base = createMockIpc({ stepMs: 50, trustAll: true });
  const api: Api = { ...base, startRun: vi.fn(base.startRun), ...overrides };
  const store = createGraphStore(api);
  void api.onRunState((s) => store.getState().applyRunState(s));
  return { api, s: () => store.getState() };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  delete win.__CG_LAUNCH__;
  vi.useRealTimers();
});

describe('mock launch_options', () => {
  it('reads window.__CG_LAUNCH__, else nulls; autorun needs a folder', async () => {
    const api = createMockIpc({ seed: false });
    expect(await api.launchOptions()).toEqual({ open_dir: null, autorun: false });
    win.__CG_LAUNCH__ = { autorun: true };
    expect(await api.launchOptions()).toEqual({ open_dir: null, autorun: false });
    win.__CG_LAUNCH__ = { open_dir: ' /g ', autorun: true };
    expect(await api.launchOptions()).toEqual({ open_dir: '/g', autorun: true });
  });
});

describe('graph store: launch', () => {
  it('without launch options init stays on the start screen', async () => {
    const { s, api } = setup();
    await s().init();
    expect(s().dir).toBeNull();
    expect(api.startRun).not.toHaveBeenCalled();
  });

  it('opens the folder without running when autorun is off', async () => {
    win.__CG_LAUNCH__ = { open_dir: MOCK_EXAMPLE_DIR };
    const { s, api } = setup();
    await s().init();
    expect(s().dir).toBe(MOCK_EXAMPLE_DIR);
    expect(api.startRun).not.toHaveBeenCalled();
  });

  it('opens and starts run all once when autorun is on and claude is found', async () => {
    win.__CG_LAUNCH__ = { open_dir: MOCK_EXAMPLE_DIR, autorun: true };
    const { s, api } = setup();
    await s().init();
    expect(api.startRun).toHaveBeenCalledTimes(1);
    expect(api.startRun).toHaveBeenCalledWith(MOCK_EXAMPLE_DIR, 'all');
    expect(s().runState?.status).toBe('running');
  });

  it('a missing folder toasts and falls back to the start screen', async () => {
    win.__CG_LAUNCH__ = { open_dir: '/mock/nope', autorun: true };
    const { s, api } = setup();
    await s().init();
    expect(s().dir).toBeNull();
    expect(lastToast(s().toasts)).toMatchObject({ kind: 'error' });
    expect(lastToast(s().toasts)?.text).toContain('/mock/nope');
    expect(api.startRun).not.toHaveBeenCalled();
  });

  it('autorun is skipped with a toast when claude is not found', async () => {
    win.__CG_LAUNCH__ = { open_dir: MOCK_EXAMPLE_DIR, autorun: true };
    const { s, api } = setup({ checkClaude: async () => ({ found: false, path: null, version: null }) });
    await s().init();
    expect(s().dir).toBe(MOCK_EXAMPLE_DIR);
    expect(api.startRun).not.toHaveBeenCalled();
    expect(lastToast(s().toasts)?.text).toMatch(/Autorun skipped/);
  });

  it('a failing launch_options leaves the start screen', async () => {
    const { s } = setup({ launchOptions: async () => Promise.reject({ code: 'ipc', message: 'x' }) });
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    await s().init();
    expect(s().dir).toBeNull();
    warn.mockRestore();
  });
});

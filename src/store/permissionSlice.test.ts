/** Permission prompt queue in the graph store: order, dedupe, merge, answers, errors. */
import { describe, expect, it, vi } from 'vitest';
import type { PermissionPrompt, RunState } from '../types/contract';
import type { Api } from '../lib/api';
import { createMockIpc } from '../lib/ipc.mock';
import { createGraphStore } from './graphStore';
import { approvalsText, mergePrompts, waitingNodeIds } from './permissionSlice';

const prompt = (id: string, node_id = 'survey', run_id = 'r1'): PermissionPrompt => ({
  id,
  run_id,
  node_id,
  tool_name: 'Bash',
  input: { command: `echo ${id}` },
  summary: `echo ${id}`,
  suggestions: [],
  ts: '2026-11-07T10:15:00Z',
});

function setup(overrides: Partial<Api> = {}) {
  const api: Api = { ...createMockIpc({ seed: false }), ...overrides };
  return createGraphStore(api, { autosaveMs: 60_000 });
}

describe('permission prompt queue', () => {
  it('keeps arrival order (oldest first), ignores duplicates and malformed payloads', () => {
    const store = setup();
    const s = store.getState;
    s().addPermissionPrompt(prompt('a'));
    s().addPermissionPrompt(prompt('b'));
    s().addPermissionPrompt(prompt('a'));
    s().addPermissionPrompt({ id: 'bad' });
    expect(s().permissionPrompts.map((p) => p.id)).toEqual(['a', 'b']);
  });

  it('defaults missing suggestions to []', () => {
    const store = setup();
    const { suggestions, ...rest } = prompt('a');
    void suggestions;
    store.getState().addPermissionPrompt(rest);
    expect(store.getState().permissionPrompts[0]?.suggestions).toEqual([]);
  });

  it('removes on permission_answered and never resurrects a resolved id', () => {
    const store = setup();
    const s = store.getState;
    s().addPermissionPrompt(prompt('a'));
    const before = s().permissionPrompts;
    s().removePermissionPrompt('a');
    expect(s().permissionPrompts).toEqual([]);
    expect(before.map((p) => p.id)).toEqual(['a']);
    s().addPermissionPrompt(prompt('a'));
    expect(s().permissionPrompts).toEqual([]);
  });

  it('a new prompt shows a hidden dialog again', () => {
    const store = setup();
    const s = store.getState;
    s().addPermissionPrompt(prompt('a'));
    s().hidePermissionDialog();
    expect(s().permissionDialogHidden).toBe(true);
    s().addPermissionPrompt(prompt('b'));
    expect(s().permissionDialogHidden).toBe(false);
    s().hidePermissionDialog();
    s().showPermissionDialog();
    expect(s().permissionDialogHidden).toBe(false);
  });

  it('load merges the list with prompts that arrived meanwhile, minus resolved ones', async () => {
    const store = setup({ listPermissionPrompts: async () => [prompt('a'), prompt('b'), prompt('gone')] });
    const s = store.getState;
    s().addPermissionPrompt(prompt('c'));
    s().removePermissionPrompt('gone');
    await s().loadPermissionPrompts();
    expect(s().permissionPrompts.map((p) => p.id)).toEqual(['a', 'b', 'c']);
  });

  it('a failing load becomes an error toast', async () => {
    const store = setup({ listPermissionPrompts: async () => Promise.reject({ code: 'io', message: 'boom' }) });
    await store.getState().loadPermissionPrompts();
    expect(store.getState().toasts.slice(-1)[0]).toMatchObject({ kind: 'error', text: expect.stringContaining('boom') });
  });

  it('answer sends the answer and drops the prompt; one answer in flight at a time', async () => {
    let release: () => void = () => {};
    const answerPermission = vi.fn(() => new Promise<void>((r) => (release = r)));
    const store = setup({ answerPermission });
    const s = store.getState;
    s().addPermissionPrompt(prompt('a'));
    const first = s().answerPermission({ id: 'a', allow: true, scope: 'node' });
    expect(s().permissionAnswering).toBe('a');
    expect(await s().answerPermission({ id: 'a', allow: false, scope: 'once' })).toBe(false);
    release();
    expect(await first).toBe(true);
    expect(answerPermission).toHaveBeenCalledTimes(1);
    expect(answerPermission).toHaveBeenCalledWith({ id: 'a', allow: true, scope: 'node' });
    expect(s().permissionPrompts).toEqual([]);
    expect(s().permissionAnswering).toBeNull();
  });

  it('not_found drops the prompt with an info toast; other errors keep it and toast', async () => {
    const codes = ['not_found', 'internal'];
    const store = setup({ answerPermission: async () => Promise.reject({ code: codes.shift(), message: 'x' }) });
    const s = store.getState;
    s().addPermissionPrompt(prompt('a'));
    s().addPermissionPrompt(prompt('b'));
    expect(await s().answerPermission({ id: 'a', allow: true, scope: 'once' })).toBe(true);
    expect(s().toasts.slice(-1)[0]?.kind).toBe('info');
    expect(await s().answerPermission({ id: 'b', allow: true, scope: 'once' })).toBe(false);
    expect(s().toasts.slice(-1)[0]).toMatchObject({ kind: 'error', text: expect.stringContaining('Could not send your answer') });
    expect(s().permissionPrompts.map((p) => p.id)).toEqual(['b']);
  });

  it('init loads the pending prompts', async () => {
    const store = setup({ listPermissionPrompts: async () => [prompt('a')] });
    await store.getState().init();
    expect(store.getState().permissionPrompts.map((p) => p.id)).toEqual(['a']);
  });
});

describe('permission helpers', () => {
  it('mergePrompts keeps list order first', () => {
    expect(mergePrompts([prompt('x'), prompt('a')], [prompt('a'), prompt('b')], new Set()).map((p) => p.id)).toEqual(['a', 'b', 'x']);
  });

  it('waitingNodeIds only counts prompts of the shown run', () => {
    const run = { run_id: 'r1' } as RunState;
    expect([...waitingNodeIds([prompt('a', 'survey'), prompt('b', 'design', 'r0')], run)]).toEqual(['survey']);
    expect(waitingNodeIds([prompt('a')], null).size).toBe(0);
  });

  it('approvalsText', () => {
    expect(approvalsText(1)).toBe('1 approval needed');
    expect(approvalsText(3)).toBe('3 approvals needed');
  });
});

/** Harness profiles: validation rules, form text mapping, session split, graph defaults, mock backend. */
import { describe, expect, it } from 'vitest';
import example from '../../examples/hello-pipeline/graph.json';
import type { AppError, Graph, HarnessProfile } from '../types/contract';
import { effectiveHarness, harnessOf, sessionHarness, sessionsOf } from '../types/contract';
import { graphToFlowNodes } from './flow';
import { addNode, connect, emptyGraph, updateNode, updateSettings } from './graphOps';
import {
  BUILTIN_HARNESSES,
  fromDraft,
  harnessLabel,
  isClaudeKind,
  isDshHarness,
  newCustomHarness,
  nextCustomId,
  parseEnv,
  toDraft,
} from './harness';
import { createMockIpc, MOCK_EXAMPLE_DIR } from './ipc.mock';
import { MOCK_HARNESS_FAIL } from './mockHarness';
import { groupSessions } from './sessions';
import { envKeyError, harnessIdError, harnessProfileError, optionPatternError, validateGraph } from './validate';

const custom = (patch: Partial<HarnessProfile> = {}): HarnessProfile => ({ ...newCustomHarness('my-cli'), command: 'my-cli', ...patch });

describe('harness validation (mirror of save_harness)', () => {
  it('ids: lower-case letters, digits and -, first not -, max 32', () => {
    for (const ok of ['claude', 'claude-deepseek', 'a', '9x', 'a'.repeat(32)]) expect(harnessIdError(ok)).toBeNull();
    for (const bad of ['', '-a', 'A', 'my_cli', 'a b', 'a'.repeat(33), 'é']) expect(harnessIdError(bad)).not.toBeNull();
  });
  it('env keys: shell variable names', () => {
    for (const ok of ['A', '_x', 'ANTHROPIC_BASE_URL', 'a1']) expect(envKeyError(ok)).toBeNull();
    for (const bad of ['', '1A', 'A-B', 'A B', 'A=B']) expect(envKeyError(bad)).not.toBeNull();
  });
  it('profile: custom needs a command; UNC refused; quiet_ms positive', () => {
    expect(harnessProfileError(custom())).toBeNull();
    expect(harnessProfileError(custom({ command: null }))).toMatch(/^command:/);
    expect(harnessProfileError(custom({ command: '\\\\server\\x.exe' }))).toMatch(/UNC/);
    expect(harnessProfileError(custom({ quiet_ms: 0 }))).toMatch(/^quiet_ms:/);
    expect(harnessProfileError(custom({ env: { 'BAD-KEY': 'x' } }))).toMatch(/^env:/);
    expect(harnessProfileError(custom({ label: ' ' }))).toMatch(/^label:/);
    expect(harnessProfileError(custom({ id: 'Bad' }))).toMatch(/^id:/);
    // Built-in claude kinds need no command.
    BUILTIN_HARNESSES.forEach((b) => expect(harnessProfileError(b)).toBeNull());
  });
});

describe('harness form text', () => {
  it('parses KEY=value lines, keeping = and ${VAR} in values', () => {
    expect(parseEnv('A=1\n\n  B = x=y \nTOKEN=${DEEPSEEK_API_KEY}')).toEqual({
      env: { A: '1', B: ' x=y', TOKEN: '${DEEPSEEK_API_KEY}' },
      error: null,
    });
    expect(parseEnv('novalue').error).toMatch(/line 1: write KEY=value/);
    expect(parseEnv('A=1\nA=2').error).toMatch(/set twice/);
    expect(parseEnv('1A=x').error).toMatch(/line 1/);
  });
  it('round-trips every built-in through the draft unchanged', () => {
    for (const b of BUILTIN_HARNESSES) expect(fromDraft(toDraft(b), true)).toEqual({ profile: b, error: null });
  });
  it('custom fields: args and markers one per line; non-custom drops them', () => {
    const d = { ...toDraft(custom()), args: ' --a \n\n--b', dialog_markers: 'Trust?\nAllow?', idle_marker: '> ', quiet_ms: '500' };
    expect(fromDraft(d, false).profile).toMatchObject({ args: ['--a', '--b'], dialog_markers: ['Trust?', 'Allow?'], idle_marker: '>', quiet_ms: 500 });
    expect(fromDraft({ ...d, kind: 'codex' }, false).profile).toMatchObject({ dialog_markers: [], idle_marker: null, quiet_ms: null });
    expect(fromDraft({ ...d, quiet_ms: '1.5' }, false).error).toMatch(/^quiet_ms:/);
    expect(fromDraft({ ...d, env: 'x' }, false).error).toMatch(/^env:/);
  });
  it('labels, kinds and new ids', () => {
    expect(harnessLabel(BUILTIN_HARNESSES, 'codex')).toBe('Codex CLI');
    expect(harnessLabel(BUILTIN_HARNESSES, 'gone')).toBe('gone');
    expect(isClaudeKind(BUILTIN_HARNESSES, 'claude-deepseek')).toBe(true);
    expect(isClaudeKind(BUILTIN_HARNESSES, 'codex')).toBe(false);
    expect(nextCustomId([...BUILTIN_HARNESSES, custom({ id: 'custom-1' })])).toBe('custom-2');
  });
  it('the DeepSeek built-in reads the key from ${DEEPSEEK_API_KEY}', () => {
    const ds = BUILTIN_HARNESSES.find((b) => b.id === 'claude-deepseek');
    expect(ds?.env.ANTHROPIC_AUTH_TOKEN).toBe('${DEEPSEEK_API_KEY}');
    expect(ds?.env.ANTHROPIC_BASE_URL).toBe('https://api.deepseek.com/anthropic');
    expect(Object.keys(ds?.env ?? {})).toHaveLength(7);
  });
});

describe('graph fields', () => {
  it('settings.harness defaults to claude, node.harness to null; unknown ids load fine', () => {
    const g = validateGraph(example) as Graph;
    expect(g.settings.harness).toBe('claude');
    expect(g.nodes.every((n) => n.harness === null)).toBe(true);
    const odd = validateGraph({ version: 1, name: 'x', settings: { harness: 'nowhere' }, nodes: [{ id: 'a', harness: 'gone' }] }) as Graph;
    expect(odd.settings.harness).toBe('nowhere');
    expect(odd.nodes[0]?.harness).toBe('gone');
    expect(validateGraph({ version: 1, name: 'x', nodes: [{ id: 'a', harness: 3 }] })).toBeInstanceOf(Error);
    expect(emptyGraph('e').settings.harness).toBe('claude');
  });
  it('effective harness = node override, else graph default; sessions of old runs are claude', () => {
    expect(effectiveHarness({ harness: null }, { harness: 'codex' })).toBe('codex');
    expect(effectiveHarness({ harness: 'claude' }, { harness: 'codex' })).toBe('claude');
    expect(harnessOf({})).toBe('claude');
    expect(sessionHarness(sessionsOf({ sessions: [{ sid: 's1', title: 't', node_ids: [], status: 'done' }] })[0])).toBe('claude');
  });
  it('node card data carries the label only when the node overrides the default', () => {
    let g = validateGraph(example) as Graph;
    g = updateNode(updateNode(g, 'design', { harness: 'codex' }), 'report', { harness: 'claude' });
    const labels = graphToFlowNodes(g, null, new Set(), (id) => harnessLabel(BUILTIN_HARNESSES, id)).map((n) => n.data.harnessLabel);
    expect(labels).toEqual([null, 'Codex CLI', null]);
  });
});

describe('groupSessions splits a chain where the harness changes', () => {
  function chain(): Graph {
    let g = emptyGraph('g');
    for (const [t, x] of [['A', 0], ['B', 200], ['C', 400], ['D', 600]] as const) g = addNode(g, t, { x, y: 0 }).graph;
    for (const [f, t] of [['a', 'b'], ['b', 'c'], ['c', 'd']] as const) {
      const r = connect(g, f, t);
      if (r.ok) g = r.graph;
    }
    return g;
  }
  it('one session while every node inherits', () => {
    const s = groupSessions(chain(), ['a', 'b', 'c', 'd']);
    expect(s.map((x) => [x.node_ids, x.harness])).toEqual([[['a', 'b', 'c', 'd'], 'claude']]);
  });
  it('maximal runs of the same effective harness; non-claude titles carry the label', () => {
    let g = updateNode(updateNode(chain(), 'b', { harness: 'codex' }), 'c', { harness: 'codex' });
    const label = (id: string): string => harnessLabel(BUILTIN_HARNESSES, id);
    expect(groupSessions(g, ['a', 'b', 'c', 'd'], label).map((x) => [x.sid, x.node_ids, x.harness, x.title])).toEqual([
      ['s1', ['a'], 'claude', 'A'],
      ['s2', ['b', 'c'], 'codex', 'B (Codex CLI)'],
      ['s3', ['d'], 'claude', 'D'],
    ]);
    g = updateSettings(g, { harness: 'codex' });
    expect(groupSessions(g, ['a', 'b', 'c', 'd'], label).map((x) => x.harness)).toEqual(['codex']);
  });
});

describe('mock harness commands', () => {
  const setup = () => createMockIpc({ trustAll: true, stepMs: 1 });

  it('dsh fields round-trip through the form; option patterns are checked', () => {
    const dsh = BUILTIN_HARNESSES.find((p) => p.id === 'deepseek-harness') as HarnessProfile;
    const draft = { ...toDraft(dsh), dsh_profile: ' headless ', option_pattern: 'ignored' };
    const { profile } = fromDraft(draft, true);
    expect(profile).toMatchObject({ kind: 'deepseek-harness', dsh_profile: 'headless', option_pattern: null });
    expect(isDshHarness(BUILTIN_HARNESSES, 'deepseek-harness')).toBe(true);
    expect(isDshHarness(BUILTIN_HARNESSES, 'codex')).toBe(false);
    expect(harnessProfileError({ ...dsh, dsh_profile: 'Bad Name' })).toMatch(/^dsh_profile/);
    expect(optionPatternError('^(?P<num>\d)\. (?P<label>.+)$')).toBeNull();
    expect(optionPatternError('^(\d)\. (.+)$')).toMatch(/label/);
    expect(optionPatternError('(?P<label>[')).toMatch(/valid/);
  });

  it('lists the four built-ins', async () => {
    const api = setup();
    expect((await api.listHarnesses()).map((p) => [p.id, p.kind, p.builtin])).toEqual([
      ['claude', 'claude', true],
      ['claude-deepseek', 'claude', true],
      ['codex', 'codex', true],
      ['deepseek-harness', 'deepseek-harness', true],
    ]);
  });
  it('saves customs and built-in overrides (label and kind of a built-in stay)', async () => {
    const api = setup();
    const after = await api.saveHarness(custom());
    expect(after.map((p) => p.id)).toEqual(['claude', 'claude-deepseek', 'codex', 'deepseek-harness', 'my-cli']);
    expect(after[4]?.builtin).toBe(false);
    const codex = { ...(after[2] as HarnessProfile), label: 'Renamed', kind: 'custom' as const, model: 'gpt-5' };
    const next = await api.saveHarness(codex);
    expect(next[2]).toMatchObject({ id: 'codex', label: 'Codex CLI', kind: 'codex', model: 'gpt-5', builtin: true });
    await expect(api.saveHarness(custom({ env: { 'NO-DASH': '1' } }))).rejects.toMatchObject({ code: 'invalid_input' });
  });
  it('deletes customs only; check reports a version or a message', async () => {
    const api = setup();
    await api.saveHarness(custom());
    await expect(api.deleteHarness('claude')).rejects.toMatchObject({ code: 'invalid_input' });
    await expect(api.deleteHarness('nope')).rejects.toMatchObject({ code: 'not_found' });
    expect(await api.checkHarness('my-cli')).toMatchObject({ id: 'my-cli', ok: true, message: null });
    expect((await api.deleteHarness('my-cli')).map((p) => p.id)).not.toContain('my-cli');
    await api.saveHarness(custom({ id: 'gone', command: `${MOCK_HARNESS_FAIL}.exe` }));
    expect(await api.checkHarness('gone')).toMatchObject({ ok: false, version: null });
  });
  it('start_run refuses a harness not defined on this machine, naming the node', async () => {
    const api = setup();
    const g = await api.openGraph(MOCK_EXAMPLE_DIR);
    await api.saveGraph(MOCK_EXAMPLE_DIR, updateNode(g, 'design', { harness: 'ghost' }));
    const err = (await api.startRun(MOCK_EXAMPLE_DIR, 'all').catch((e: AppError) => e)) as AppError;
    expect(err.code).toBe('invalid_input');
    expect(err.message).toMatch(/design.*harness ghost is not defined on this machine; add it under Settings → Harnesses/);
  });
});

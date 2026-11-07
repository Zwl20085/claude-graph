/** Tests for the fix pass: field rules, trust items, tool parsing, RF selection, message merge, IPC helpers. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import example from '../../examples/hello-pipeline/graph.json';
import { isRunState, type Graph, type NodeMessage, type TrustItem } from '../types/contract';
import { committedPositions, selectOnly } from './flow';
import { parseToolList, readToolList, splitTopLevel, updateNode, updateSettings } from './graphOps';
import { describeError } from './ipc';
import { createMockIpc, MOCK_EXAMPLE_DIR } from './ipc.mock';
import { createMessageHistory, mergeMessages } from './messages';
import { trustFingerprint, trustItems, trustItemText } from './trust';
import {
  claudeBinError,
  continueSessionError,
  deliverablePathError,
  isAbsolutePath,
  nodeIdError,
  toolEntryError,
  validateGraph,
  workingDirError,
} from './validate';

const load = (): Graph => {
  const g = validateGraph(example);
  if (g instanceof Error) throw g;
  return g;
};

describe('node id rule', () => {
  it.each(['a', 'A1', 'x.y', 'a_b-c', '9', 'a'.repeat(128)])('accepts %s', (id) => {
    expect(nodeIdError(id)).toBeNull();
  });
  it.each([
    ['', /letter or digit/],
    ['-a', /letter or digit/],
    ['a b', /letter or digit/],
    ['a'.repeat(129), /max 128/],
    ['a.', /dot/],
    ['CON', /reserved/],
    ['nul.txt', /reserved/],
    ['Lpt9', /reserved/],
  ])('rejects %s', (id, pattern) => {
    expect(nodeIdError(id)).toMatch(pattern);
  });
  it('is unique case-insensitively', () => {
    expect(nodeIdError('Survey', ['survey'])).toMatch(/already used/);
    expect(nodeIdError('com10')).toBeNull();
  });
});

describe('allowed_tools entry rule', () => {
  it.each(['Read', 'Bash(python:*)', 'Bash(git add, git commit)', 'mcp__srv', 'mcp__srv__tool', 'Web_Fetch2'])('accepts %s', (t) => {
    expect(toolEntryError(t)).toBeNull();
  });
  it.each(['-x', '--dangerously', '1Read', 'Bash(a & b)', 'Bash(a|b)', 'Bash("x")', 'Read Write', 'Bash(x', 'Bash(%PATH%)'])(
    'rejects %s',
    (t) => {
      expect(toolEntryError(t)).not.toBeNull();
    },
  );
  it('rejects control characters inside parentheses', () => {
    expect(toolEntryError('Bash(a\nb)')).not.toBeNull();
  });
});

describe('path rules', () => {
  it('deliverable paths stay relative', () => {
    expect(deliverablePathError('out/report.md')).toBeNull();
    expect(deliverablePathError('')).toMatch(/empty/);
    expect(deliverablePathError('../x')).toMatch(/\.\./);
    expect(deliverablePathError('a\\..\\b')).toMatch(/\.\./);
    expect(deliverablePathError('/etc/x')).toMatch(/relative/);
    expect(deliverablePathError('\\x')).toMatch(/relative/);
    expect(deliverablePathError('C:x')).toMatch(/drive/);
  });

  it('working_dir: null, relative-safe or absolute; never UNC or ..', () => {
    expect(workingDirError(null)).toBeNull();
    expect(workingDirError('sub/dir')).toBeNull();
    expect(workingDirError('C:\\work')).toBeNull();
    expect(workingDirError('/home/u')).toBeNull();
    expect(workingDirError('C:\\work\\..\\x')).toMatch(/\.\./);
    expect(workingDirError('\\\\server\\share')).toMatch(/UNC/);
    expect(workingDirError('//server/share')).toMatch(/UNC/);
    expect(workingDirError('C:rel')).toMatch(/drive/);
    expect(isAbsolutePath('D:/x')).toBe(true);
    expect(isAbsolutePath('x')).toBe(false);
  });

  it.each(['\\\\server\\share', '\\\\server\\share\\sub', '\\\\?\\UNC\\server\\share\\x', '//server/share'])(
    'working_dir rejects UNC path %s (same as graph_util::validate)',
    (dir) => {
      expect(workingDirError(dir)).toMatch(/UNC/);
      expect(validateGraph({ ...load(), nodes: load().nodes.map((n, i) => (i === 0 ? { ...n, working_dir: dir } : n)) })).toBeInstanceOf(Error);
    },
  );

  it('continue_session_of must be a direct upstream', () => {
    expect(continueSessionError(null, [])).toBeNull();
    expect(continueSessionError('a', ['a'])).toBeNull();
    expect(continueSessionError('b', ['a'])).toMatch(/upstream/);
  });

  it('claude path shape', () => {
    expect(claudeBinError('C:\\tools\\claude.exe')).toBeNull();
    expect(claudeBinError('C:/npm/claude.cmd')).toBeNull();
    expect(claudeBinError('/usr/local/bin/claude')).toBeNull();
    expect(claudeBinError('C:/x/claude-code.exe')).toBeNull();
    expect(claudeBinError('claude')).toMatch(/absolute/);
    expect(claudeBinError('\\\\srv\\claude.exe')).toMatch(/UNC/);
    expect(claudeBinError('C:/x/evil.exe')).toMatch(/claude/);
    expect(claudeBinError('C:/x/claude-code.bat')).toMatch(/claude/);
  });

  it('validateGraph applies the node rules (mock save_graph mirrors the backend)', () => {
    const g = load();
    const bad = (patch: Parameters<typeof updateNode>[2]) => validateGraph(updateNode(g, 'design', patch));
    expect(bad({ allowed_tools: ['-x'] })).toBeInstanceOf(Error);
    expect(bad({ working_dir: '../up' })).toBeInstanceOf(Error);
    expect(bad({ deliverables: [{ path: '/abs', description: '' }] })).toBeInstanceOf(Error);
    expect((bad({ continue_session_of: 'report' }) as Error).message).toMatch(/continue_session_of/);
    expect(validateGraph({ ...g, nodes: [...g.nodes, { ...g.nodes[0], id: 'SURVEY' }] })).toBeInstanceOf(Error);
    expect(bad({ working_dir: 'C:\\abs', continue_session_of: 'survey' })).not.toBeInstanceOf(Error);
  });
});

describe('tool list parsing (M8)', () => {
  it('splits on commas only at parenthesis depth 0', () => {
    expect(splitTopLevel('Bash(git add, git commit), Read')).toEqual(['Bash(git add, git commit)', 'Read']);
    expect(parseToolList('Read, Bash(a,(b,c)),Write')).toEqual(['Read', 'Bash(a,(b,c))', 'Write']);
    expect(splitTopLevel('a), b')).toEqual(['a)', 'b']);
    expect(parseToolList(' , ')).toBeNull();
  });

  it('readToolList commits the valid entries and reports the invalid ones', () => {
    expect(readToolList('Read, Bash(x)')).toEqual({ valid: ['Read', 'Bash(x)'], error: null });
    const r = readToolList('Read, -bad, 1x');
    expect(r.valid).toEqual(['Read']);
    expect(r.error).toMatch(/-bad.*\+1 more.*not saved/);
    expect(readToolList('-only').valid).toBeNull();
  });
});

describe('trust items', () => {
  it('the example has one item: Bash on design', () => {
    expect(trustItems(load())).toEqual([{ node_id: 'design', kind: 'bash_tool', detail: 'Bash(python:*)' }]);
  });

  it('derives every kind and renders human text', () => {
    let g = updateSettings(load(), { default_permission_mode: 'dontAsk', claude_bin: '/x/claude' });
    g = updateNode(g, 'survey', { permission_mode: 'bypassPermissions', working_dir: 'C:\\w' });
    g = updateNode(g, 'report', { permission_mode: 'dontAsk' });
    const items = trustItems(g);
    expect(items.map((i) => [i.node_id, i.kind])).toEqual([
      [null, 'dont_ask'],
      [null, 'claude_bin_in_graph'],
      ['survey', 'bypass_permissions'],
      ['survey', 'absolute_working_dir'],
      ['design', 'bash_tool'],
      ['report', 'dont_ask'],
    ]);
    const texts = items.map(trustItemText);
    expect(texts).toContain('Default permission mode is dontAsk (Claude runs its allowed tools without asking)');
    expect(texts).toContain('graph.json names a claude binary (ignored)');
    expect(texts).toContain('Node survey runs with bypassPermissions (Claude may run any command)');
    expect(texts).toContain('Node survey uses an absolute working directory: C:\\w');
    expect(texts).toContain('Node design allows Bash tools: Bash(python:*)');
    expect(texts).toContain('Node report runs with dontAsk (Claude runs its allowed tools without asking)');
    const more: TrustItem[] = [
      { node_id: null, kind: 'bypass_permissions', detail: '' },
      { node_id: null, kind: 'bash_tool', detail: '' },
      { node_id: null, kind: 'absolute_working_dir', detail: '/x' },
      { node_id: 'n', kind: 'future_kind' as TrustItem['kind'], detail: 'd' },
    ];
    expect(more.map(trustItemText)).toEqual([
      'Default permission mode is bypassPermissions (Claude may run any command)',
      'A node allows Bash tools',
      'A node uses an absolute working directory: /x',
      'Node n: future_kind d',
    ]);
    expect(trustFingerprint(items)).not.toBe(trustFingerprint(items.slice(1)));
  });
});

describe('mock trust, app settings, reveal', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('start_run is refused until trusted; a changed elevated field needs trust again', async () => {
    const api = createMockIpc({ stepMs: 10 });
    const status = await api.graphTrustStatus(MOCK_EXAMPLE_DIR);
    expect(status.trusted).toBe(false);
    await expect(api.startRun(MOCK_EXAMPLE_DIR, 'all')).rejects.toMatchObject({ code: 'untrusted_graph' });
    await api.trustGraph(MOCK_EXAMPLE_DIR);
    expect((await api.graphTrustStatus(MOCK_EXAMPLE_DIR)).trusted).toBe(true);
    const g = await api.openGraph(MOCK_EXAMPLE_DIR);
    await api.saveGraph(MOCK_EXAMPLE_DIR, updateNode(g, 'report', { permission_mode: 'bypassPermissions' }));
    expect((await api.graphTrustStatus(MOCK_EXAMPLE_DIR)).trusted).toBe(false);
    await api.createGraph('/mock/plain', 'plain');
    expect(await api.graphTrustStatus('/mock/plain')).toEqual({ trusted: true, items: [] });
  });

  it('app settings round-trip with validation; clearing is allowed', async () => {
    const api = createMockIpc({ seed: false });
    expect(await api.getAppSettings()).toEqual({ claude_bin: null });
    await expect(api.setAppSettings({ claude_bin: 'x' })).rejects.toMatchObject({ code: 'invalid_input' });
    expect(await api.setAppSettings({ claude_bin: ' /opt/claude ' })).toEqual({ claude_bin: '/opt/claude' });
    expect((await api.checkClaude()).path).toBe('/opt/claude');
    expect(await api.setAppSettings({ claude_bin: '' })).toEqual({ claude_bin: null });
  });
});

describe('RF selection and committed moves (M1, M2)', () => {
  const ns = [
    { id: 'a', selected: true },
    { id: 'b', selected: true },
    { id: 'c', selected: false },
  ];
  it('selectOnly keeps a user multi-select that contains the store id', () => {
    expect(selectOnly(ns, 'a')).toBe(ns);
    expect(selectOnly(ns, 'c').map((n) => n.selected)).toEqual([false, false, true]);
    expect(selectOnly(ns, null).every((n) => !n.selected)).toBe(true);
    const none = [{ id: 'a', selected: false }];
    expect(selectOnly(none, null)).toBe(none);
  });

  it('committedPositions takes only non-dragging position changes', () => {
    expect(
      committedPositions([
        { type: 'position', id: 'a', position: { x: 1, y: 2 }, dragging: false },
        { type: 'position', id: 'b', position: { x: 3, y: 4 }, dragging: true },
        { type: 'position', id: 'c', dragging: false },
        { type: 'select', id: 'd' },
      ]),
    ).toEqual({ a: { x: 1, y: 2 } });
  });
});

describe('messages merge (M5)', () => {
  const m = (role: NodeMessage['role'], text: string, ts: string): NodeMessage => ({ role, text, ts });

  it('dedupes on (role, ts, text) and keeps live entries newer than the snapshot', () => {
    const snap = [m('user', 'hi', '1'), m('assistant', 'yo', '2')];
    expect(mergeMessages(snap, [m('user', 'hi', '1'), m('user', 'more', '3')]).map((x) => x.text)).toEqual(['hi', 'yo', 'more']);
    // An older live copy of a snapshot message (other ts) is dropped; a newer one with the same text is kept.
    expect(mergeMessages(snap, [m('user', 'hi', '0')])).toHaveLength(2);
    expect(mergeMessages(snap, [m('user', 'hi', '5')])).toHaveLength(3);
    expect(mergeMessages([], [m('user', 'a', '1'), m('user', 'a', '1')])).toHaveLength(1);
  });

  it('a snapshot that resolves after a live entry does not drop it; stale refreshes are ignored', async () => {
    const replies: Array<(v: NodeMessage[]) => void> = [];
    const list = () => new Promise<NodeMessage[]>((r) => replies.push(r));
    const seen: NodeMessage[][] = [];
    const h = createMessageHistory(list, (msgs) => seen.push(msgs));
    const first = h.refresh();
    h.addLive([m('user', 'sent live', '9')]);
    h.addLive([]);
    const second = h.refresh();
    replies[1]?.([m('assistant', 'reply', '8')]);
    await second;
    replies[0]?.([]);
    await first;
    expect(h.current().map((x) => x.text)).toEqual(['reply', 'sent live']);
    expect(seen[seen.length - 1]).toEqual(h.current());
    h.dispose();
    h.addLive([m('user', 'late', '10')]);
    expect(seen).toHaveLength(2);
  });

  it('reports list errors', async () => {
    const onError = vi.fn();
    const h = createMessageHistory(() => Promise.reject(new Error('x')), () => undefined, onError);
    await h.refresh();
    expect(onError).toHaveBeenCalledTimes(1);
  });
});

describe('IPC helpers', () => {
  it('describeError: Error, string, JSON, and String fallback', () => {
    expect(describeError(new Error('e'))).toBe('e');
    expect(describeError('s')).toBe('s');
    expect(describeError({ a: 1 })).toBe('{"a":1}');
    const circular: Record<string, unknown> = {};
    circular.self = circular;
    expect(describeError(circular)).toBe('[object Object]');
    expect(describeError(undefined)).toBe('undefined');
  });

  it('isRunState guards event payloads', () => {
    const ok = { run_id: 'r', graph_dir: '/g', started_at: 't', finished_at: null, status: 'running', nodes: { a: { status: 'queued', reason: null } } };
    expect(isRunState(ok)).toBe(true);
    expect(isRunState({ ...ok, status: 'weird' })).toBe(false);
    expect(isRunState({ ...ok, nodes: { a: { status: 'x' } } })).toBe(false);
    expect(isRunState({ ...ok, finished_at: 3 })).toBe(false);
    expect(isRunState(null)).toBe(false);
    expect(isRunState([])).toBe(false);
  });
});

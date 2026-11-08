/** Turn-outcome slice: prompts from events and lists, answers, continue, failures, toasts. */
import { describe, expect, it, vi } from 'vitest';
import type { RunState, SessionPrompt } from '../types/contract';
import type { Api } from '../lib/api';
import { createMockIpc } from '../lib/ipc.mock';
import { ringStyle, STATUS_COLORS, statusLabel, WAITING_LABEL } from '../lib/status';
import { createGraphStore } from './graphStore';
import { answerFailed, needsYouText, promptOfNode } from './promptSlice';

const reply = (id = 'a-p1', node_id = 'a'): SessionPrompt => ({
  run_id: 'R1',
  sid: 's1',
  node_id,
  prompt: { type: 'reply', id, question: 'Which framework?', choices: [{ label: 'vitest' }], reply_excerpt: 'Which framework?' },
});

const run = (pending: string | null): RunState => ({
  run_id: 'R1',
  graph_dir: '/g',
  started_at: 't',
  finished_at: null,
  status: 'running',
  nodes: {},
  mode: 'terminal',
  sessions: [{ sid: 's1', title: 'T', node_ids: ['a'], status: 'running', pid: 1, current_node: 'a', pending_prompt: pending }],
});

function setup(overrides: Partial<Api> = {}) {
  const api: Api = { ...createMockIpc({ seed: false }), ...overrides };
  return { store: createGraphStore(api, { autosaveMs: 60_000 }), api };
}

describe('waiting status', () => {
  it('is blue, pulsing and labelled "needs you"', () => {
    expect(STATUS_COLORS.waiting).toBe('#38bdf8');
    expect(ringStyle('waiting').className).toBe('cg-pulse-wait');
    expect(statusLabel('waiting')).toBe(WAITING_LABEL);
  });
});

describe('session prompts', () => {
  it('an event adds the prompt once and raises a toast that names the node', () => {
    const { store } = setup();
    const s = store.getState;
    s().addSessionPrompt(reply());
    s().addSessionPrompt(reply());
    s().addSessionPrompt({ bad: true });
    expect(s().sessionPrompts).toHaveLength(1);
    const toast = s().toasts[s().toasts.length - 1];
    expect(toast?.nodeId).toBe('a');
    expect(toast?.text).toBe(needsYouText(reply()));
    expect(promptOfNode(s().sessionPrompts, run('a-p1'), 'a')?.prompt.id).toBe('a-p1');
    expect(promptOfNode(s().sessionPrompts, null, 'a')).toBeNull();
    s().clearSessionPrompt({ run_id: 'R1', sid: 's1', node_id: 'a', id: 'a-p1', via: 'window' });
    expect(s().sessionPrompts).toEqual([]);
  });

  it('a new prompt of a session replaces its old one', () => {
    const { store } = setup();
    store.getState().addSessionPrompt(reply('a-p1'));
    store.getState().addSessionPrompt(reply('a-p2'));
    expect(store.getState().sessionPrompts.map((p) => p.prompt.id)).toEqual(['a-p2']);
  });

  it('answering disables until the prompt clears; not_found drops it', async () => {
    const answerPrompt = vi.fn().mockResolvedValue(undefined);
    const { store } = setup({ answerPrompt });
    const s = store.getState;
    s().addSessionPrompt(reply());
    expect(await s().answerPrompt(reply(), { choice: 0 })).toBe(true);
    expect(answerPrompt).toHaveBeenCalledWith('R1', 's1', 'a-p1', { choice: 0 });
    expect(s().promptAnswering).toBe('a-p1');
    expect(await s().answerPrompt(reply(), { choice: 0 })).toBe(false);
    s().clearSessionPrompt({ run_id: 'R1', sid: 's1', node_id: 'a', id: 'a-p1', via: 'app' });
    expect(s().promptAnswering).toBeNull();

    const gone = setup({ answerPrompt: vi.fn().mockRejectedValue({ code: 'not_found', message: 'gone' }) }).store;
    gone.getState().addSessionPrompt(reply());
    expect(await gone.getState().answerPrompt(reply(), { text: 'x' })).toBe(false);
    expect(gone.getState().sessionPrompts).toEqual([]);
    expect(gone.getState().promptAnswering).toBeNull();
  });

  it('continue calls continue_node; errors become toasts', async () => {
    const continueNode = vi.fn().mockRejectedValue({ code: 'invalid_input', message: 'not waiting' });
    const { store } = setup({ continueNode });
    store.getState().addSessionPrompt(reply());
    expect(await store.getState().continueNode(reply())).toBe(false);
    expect(continueNode).toHaveBeenCalledWith('R1', 's1', 'a');
    expect(store.getState().toasts[store.getState().toasts.length - 1]?.text).toBe('Could not continue: not waiting');
  });

  it('a stderr event of the answering node re-enables the card', () => {
    const ev = { kind: 'stderr', node_id: 'a', run_id: 'R1' };
    expect(answerFailed(ev, [reply()], 'a-p1')).toBe(true);
    expect(answerFailed(ev, [reply()], null)).toBe(false);
    expect(answerFailed({ ...ev, kind: 'text' }, [reply()], 'a-p1')).toBe(false);
    expect(answerFailed({ ...ev, node_id: 'b' }, [reply()], 'a-p1')).toBe(false);
  });

  it('a run whose session waits on an unknown prompt loads the list', async () => {
    const listSessionPrompts = vi.fn().mockResolvedValue([reply()]);
    const { store } = setup({ listSessionPrompts });
    await store.getState().syncSessionPrompts(run(null));
    expect(listSessionPrompts).not.toHaveBeenCalled();
    await store.getState().syncSessionPrompts(run('a-p1'));
    expect(listSessionPrompts).toHaveBeenCalledWith('R1');
    expect(store.getState().sessionPrompts).toHaveLength(1);
    await store.getState().syncSessionPrompts(run('a-p1'));
    expect(listSessionPrompts).toHaveBeenCalledTimes(1);
  });
});

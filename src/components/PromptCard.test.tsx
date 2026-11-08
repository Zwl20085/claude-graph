/** "Needs you" card, the questions list of the Results tab, and the mock demo of turn outcomes. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { QuestionRecord, RunState, SessionPrompt, TurnPrompt } from '../types/contract';
import type { Api } from '../lib/api';
import { createMockIpc } from '../lib/ipc.mock';
import { MOCK_DIALOG } from '../lib/mockRunner';
import { createGraphStore, installStore } from '../store/graphStore';
import { CONTINUE_LABEL, PromptCard } from './PromptCard';
import { answerLine, QuestionList } from './ResultsTab';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
afterEach(async () => {
  if (root) await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
});

async function render(el: JSX.Element): Promise<void> {
  const host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root?.render(el));
}
const button = (text: string): HTMLButtonElement => {
  const b = [...document.querySelectorAll('button')].find((x) => x.textContent?.includes(text));
  if (!b) throw new Error(`no button ${text}`);
  return b;
};
const click = async (el: HTMLElement): Promise<void> => act(async () => el.click());

function setup(prompt: TurnPrompt) {
  const answerPrompt = vi.fn().mockResolvedValue(undefined);
  const continueNode = vi.fn().mockResolvedValue(undefined);
  const api: Api = { ...createMockIpc({ seed: false }), answerPrompt, continueNode };
  const store = createGraphStore(api, { autosaveMs: 60_000 });
  installStore(store);
  const sp: SessionPrompt = { run_id: 'R1', sid: 's1', node_id: 'a', prompt };
  store.getState().addSessionPrompt(sp);
  return { store, sp, answerPrompt, continueNode };
}

describe('PromptCard', () => {
  it('a choice dialog: option buttons send the index; the free-text option allows typing', async () => {
    const { sp, answerPrompt, store } = setup({ ...MOCK_DIALOG, id: 'a-p1' });
    await render(<PromptCard prompt={sp} />);
    expect(document.body.textContent).toContain('Which framework should I use? (mock)');
    expect(button('jest').title).toBe('The classic.');
    expect(document.querySelector('textarea[aria-label="your answer"]')).not.toBeNull();
    expect(() => button(CONTINUE_LABEL)).toThrow();
    await click(button('jest'));
    expect(answerPrompt).toHaveBeenCalledWith('R1', 's1', 'a-p1', { choice: 1 });
    expect(store.getState().promptAnswering).toEqual(['a-p1']);
    expect(button('vitest').disabled).toBe(true);
  });

  it('a permission dialog takes no text', async () => {
    const { sp } = setup({ ...MOCK_DIALOG, id: 'a-p1', kind: 'permission', allows_text: false });
    await render(<PromptCard prompt={sp} />);
    expect(document.querySelector('textarea')).toBeNull();
    expect(document.body.textContent).toContain('asks for permission');
  });

  it('multi-select: checkboxes and Confirm send the sorted list', async () => {
    const { sp, answerPrompt } = setup({ ...MOCK_DIALOG, id: 'a-p1', multi: true });
    await render(<PromptCard prompt={sp} />);
    const boxes = [...document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
    expect(boxes).toHaveLength(3);
    expect(button('Confirm').disabled).toBe(true);
    await click(boxes[1] as HTMLInputElement);
    await click(boxes[0] as HTMLInputElement);
    await click(button('Confirm'));
    expect(answerPrompt).toHaveBeenCalledWith('R1', 's1', 'a-p1', { choices: [0, 1] });
  });

  it('a reply question offers Continue', async () => {
    const { sp, continueNode } = setup({
      type: 'reply',
      id: 'a-p2',
      question: 'Want tests too?',
      choices: [],
      reply_excerpt: 'Want tests too?',
    });
    await render(<PromptCard prompt={sp} />);
    expect(document.body.textContent).toContain('a is asking:');
    // The first skip of a run asks; declining sends nothing.
    const confirm = vi.fn().mockReturnValueOnce(false).mockReturnValue(true);
    await act(async () => root?.render(<PromptCard prompt={sp} confirm={confirm} />));
    await click(button(CONTINUE_LABEL));
    expect(continueNode).not.toHaveBeenCalled();
    await click(button(CONTINUE_LABEL));
    expect(continueNode).toHaveBeenCalledWith('R1', 's1', 'a');
    expect(confirm).toHaveBeenCalledTimes(2);
  });
});

describe('Results tab questions', () => {
  const rec = (patch: Partial<QuestionRecord>): QuestionRecord => ({
    id: 'a-p1',
    ts: 't',
    kind: 'dialog',
    source: 'screen',
    question: 'A or B?',
    options: [{ label: 'A' }, { label: 'B' }],
    ...patch,
  });

  it('says how each was answered', async () => {
    expect(answerLine(rec({}))).toBe('not answered');
    expect(answerLine(rec({ answer: { ts: 't', via: 'app', index: 1 } }))).toBe('answered from the app: B');
    expect(answerLine(rec({ answer: { ts: 't', via: 'window' } }))).toBe('answered in the window');
    expect(answerLine(rec({ answer: { ts: 't', via: 'continue' } }))).toBe('continued without answering');
    expect(answerLine(rec({ offer: true }))).toContain('offer');
    await render(<QuestionList questions={[rec({}), rec({ id: 'a-p2', kind: 'question' })]} />);
    expect(document.body.textContent).toContain('Questions (2)');
    expect(document.body.textContent).toContain('1. A / 2. B');
  });
});

describe('mock demo of turn outcomes', () => {
  it('a dialog on the first node, a reply question after the second; answers resume the run', async () => {
    vi.useFakeTimers();
    try {
      const api = createMockIpc({ stepMs: 1, trustAll: true, simulatePrompts: true, exampleRunMode: 'terminal' });
      const states: RunState[] = [];
      const prompts: SessionPrompt[] = [];
      await api.onRunState((s) => states.push(s));
      await api.onSessionPrompt((p) => prompts.push(p));
      const runId = await api.startRun('/mock/hello-pipeline', 'all');
      await vi.advanceTimersByTimeAsync(50);
      expect(prompts).toHaveLength(1);
      const first = prompts[0] as SessionPrompt;
      expect(first.prompt.type).toBe('dialog');
      expect(states[states.length - 1]?.nodes[first.node_id]?.status).toBe('waiting');
      expect(states[states.length - 1]?.sessions?.[0]?.pending_prompt).toBe(first.prompt.id);
      expect(await api.listSessionPrompts(runId)).toHaveLength(1);
      await expect(api.answerPrompt(runId, first.sid, first.prompt.id, { choices: [0] })).rejects.toMatchObject({ code: 'invalid_input' });
      await api.answerPrompt(runId, first.sid, first.prompt.id, { choice: 0 });
      await vi.advanceTimersByTimeAsync(50);
      expect(prompts).toHaveLength(2);
      const second = prompts[1] as SessionPrompt;
      expect(second.prompt.type).toBe('reply');
      await api.continueNode(runId, second.sid, second.node_id);
      await vi.advanceTimersByTimeAsync(200);
      expect(states[states.length - 1]?.status).toBe('succeeded');
    } finally {
      vi.useRealTimers();
    }
  });
});

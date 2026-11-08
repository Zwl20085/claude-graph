/** Turn-outcome contract: the shared fixture (also read by contract_prompt.rs) and the guards. */
import { describe, expect, it } from 'vitest';
import fixture from './fixtures/session_prompt.json';
import {
  isQuestionRecord,
  isSessionPrompt,
  isSessionPromptCleared,
  isTerminalSession,
  isTurnPrompt,
  NODE_EVENT_KINDS,
  NODE_STATUSES,
  promptLabels,
  promptTakesText,
  questionsOf,
  type PromptAnswer,
  type SessionPrompt,
} from './contract';

describe('turn outcome contract parity', () => {
  it('the Rust fixture passes the guards', () => {
    expect(isSessionPrompt(fixture.dialog)).toBe(true);
    expect(isSessionPrompt(fixture.reply)).toBe(true);
    expect(isSessionPromptCleared(fixture.cleared)).toBe(true);
    const answers: PromptAnswer[] = fixture.answers;
    expect(answers).toHaveLength(3);
    const dialog = fixture.dialog as SessionPrompt;
    expect(promptLabels(dialog.prompt).map((c) => c.label)).toEqual(['A', 'B', 'Type something.']);
    expect(promptTakesText(dialog.prompt)).toBe(true);
    expect(promptTakesText((fixture.reply as SessionPrompt).prompt)).toBe(true);
  });

  it('the new statuses and event kinds exist', () => {
    expect(NODE_STATUSES).toContain('waiting');
    expect(NODE_EVENT_KINDS).toEqual(expect.arrayContaining(['question', 'dialog', 'answer']));
  });

  it('guards reject malformed prompts', () => {
    const d = fixture.dialog.prompt;
    expect(isTurnPrompt({ ...d, kind: 'menu' })).toBe(false);
    expect(isTurnPrompt({ ...d, id: '' })).toBe(false);
    expect(isTurnPrompt({ ...d, options: [{ index: -1, label: 'x' }] })).toBe(false);
    expect(isTurnPrompt({ ...d, type: 'other' })).toBe(false);
    expect(isTurnPrompt({ ...fixture.reply.prompt, choices: [{}] })).toBe(false);
    expect(isSessionPrompt({ ...fixture.dialog, sid: 1 })).toBe(false);
    expect(isSessionPromptCleared({ ...fixture.cleared, via: 'mail' })).toBe(false);
    const permission = { ...d, kind: 'permission', allows_text: false };
    expect(promptTakesText(permission as SessionPrompt['prompt'])).toBe(false);
  });

  it('sessions carry pending_prompt; question records are filtered', () => {
    const s = { sid: 's1', title: 'T', node_ids: ['a'], status: 'running', pid: 1, current_node: 'a' };
    expect(isTerminalSession({ ...s, pending_prompt: 'a-p1' })).toBe(true);
    expect(isTerminalSession({ ...s, pending_prompt: null })).toBe(true);
    expect(isTerminalSession({ ...s, pending_prompt: 3 })).toBe(false);
    const rec = { id: 'a-p1', ts: 't', kind: 'question', source: 'reply', question: 'Q?', options: [] };
    expect(isQuestionRecord(rec)).toBe(true);
    expect(questionsOf({ questions: [rec, { id: 'x' }] })).toHaveLength(1);
    expect(questionsOf(null)).toEqual([]);
  });
});

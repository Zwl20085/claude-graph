/**
 * Turn-outcome slice of the graph store (TURN_CONTRACT): the questions and
 * dialogs terminal sessions wait on. Filled by `list_session_prompts` when a
 * shown run has a session with `pending_prompt`, and kept current by the
 * `session_prompt` / `session_prompt_cleared` events. A new prompt raises a
 * toast that selects its node. Every update builds a new array.
 */
import {
  isAppError,
  isSessionPrompt,
  isSessionPromptCleared,
  sessionsOf,
  type PromptAnswer,
  type RunState,
  type SessionPrompt,
} from '../types/contract';
import type { Api } from '../lib/api';
import { errorMessage } from './runHelpers';

export interface PromptState {
  /** Pending prompts of the shown runs, oldest first. */
  sessionPrompts: readonly SessionPrompt[];
  /** Id of the prompt whose answer is in flight (its card is disabled until it clears). */
  promptAnswering: string | null;
}

export interface PromptActions {
  /** Load the prompts of `run` when one of its sessions waits on one we do not know. */
  syncSessionPrompts: (run: RunState) => Promise<void>;
  /** A `session_prompt` event (validated here). */
  addSessionPrompt: (prompt: unknown) => void;
  /** A `session_prompt_cleared` event. */
  clearSessionPrompt: (cleared: unknown) => void;
  /** Returns true when the backend took the answer. */
  answerPrompt: (prompt: SessionPrompt, answer: PromptAnswer) => Promise<boolean>;
  continueNode: (prompt: SessionPrompt) => Promise<boolean>;
}

export const INITIAL_PROMPTS: PromptState = { sessionPrompts: [], promptAnswering: null };

type Toast = (kind: 'error' | 'info' | 'warning', text: string, nodeId?: string) => void;
type Get = () => PromptState;
type SetPatch = (patch: Partial<PromptState>) => void;

/** Toast text of a new prompt. */
export function needsYouText(p: SessionPrompt): string {
  const what = p.prompt.type === 'dialog' ? 'a dialog in its window' : 'a question';
  return `Node ${p.node_id} needs you: ${what} — “${p.prompt.question}”`;
}

/**
 * A stderr event of the node whose answer is in flight: the answer did not
 * land (`dialog_answer_failed`, an undelivered reply); the card is enabled again.
 */
export function answerFailed(
  ev: { kind: string; node_id: string; run_id: string },
  prompts: readonly SessionPrompt[],
  answering: string | null,
): boolean {
  if (answering === null || ev.kind !== 'stderr') return false;
  return prompts.some((p) => p.prompt.id === answering && p.node_id === ev.node_id && p.run_id === ev.run_id);
}

/** The pending prompt of a node in the shown run, or null. */
export function promptOfNode(prompts: readonly SessionPrompt[], run: RunState | null, nodeId: string): SessionPrompt | null {
  if (run === null) return null;
  return prompts.find((p) => p.run_id === run.run_id && p.node_id === nodeId) ?? null;
}

export function createPromptActions(api: Api, set: SetPatch, get: Get, toast: Toast): PromptActions {
  const without = (id: string): void => {
    const { sessionPrompts, promptAnswering } = get();
    set({
      sessionPrompts: sessionPrompts.filter((p) => p.prompt.id !== id),
      promptAnswering: promptAnswering === id ? null : promptAnswering,
    });
  };

  const add = (p: SessionPrompt, announce: boolean): void => {
    const current = get().sessionPrompts;
    if (current.some((x) => x.prompt.id === p.prompt.id)) return;
    // One prompt per session: a new one replaces the session's old one.
    const others = current.filter((x) => !(x.run_id === p.run_id && x.sid === p.sid));
    set({ sessionPrompts: [...others, p] });
    if (announce) toast('warning', needsYouText(p), p.node_id);
  };

  return {
    syncSessionPrompts: async (run) => {
      const ids = sessionsOf(run)
        .map((s) => s.pending_prompt)
        .filter((id): id is string => typeof id === 'string');
      const known = new Set(get().sessionPrompts.map((p) => p.prompt.id));
      if (ids.every((id) => known.has(id))) return;
      try {
        const listed = await api.listSessionPrompts(run.run_id);
        listed.filter(isSessionPrompt).forEach((p) => add(p, false));
      } catch (err) {
        if (!(isAppError(err) && err.code === 'not_found')) toast('error', `Could not load the questions of the run: ${errorMessage(err)}`);
      }
    },
    addSessionPrompt: (prompt) => {
      if (!isSessionPrompt(prompt)) {
        console.warn('ignored a malformed session_prompt payload', prompt);
        return;
      }
      add(prompt, true);
    },
    clearSessionPrompt: (cleared) => {
      if (isSessionPromptCleared(cleared)) without(cleared.id);
    },
    answerPrompt: async (p, answer) => {
      if (get().promptAnswering !== null) return false;
      set({ promptAnswering: p.prompt.id });
      try {
        await api.answerPrompt(p.run_id, p.sid, p.prompt.id, answer);
        return true;
      } catch (err) {
        set({ promptAnswering: null });
        if (isAppError(err) && err.code === 'not_found') {
          without(p.prompt.id);
          toast('info', 'That question was already answered.');
          return false;
        }
        toast('error', `Could not send your answer: ${errorMessage(err)}`);
        return false;
      }
    },
    continueNode: async (p) => {
      if (get().promptAnswering !== null) return false;
      set({ promptAnswering: p.prompt.id });
      try {
        await api.continueNode(p.run_id, p.sid, p.node_id);
        return true;
      } catch (err) {
        set({ promptAnswering: null });
        toast('error', `Could not continue: ${errorMessage(err)}`);
        return false;
      }
    },
  };
}

/**
 * Turn-outcome slice of the graph store (TURN_CONTRACT): the questions and
 * dialogs terminal sessions wait on. Filled by `list_session_prompts` when a
 * shown run has a session with `pending_prompt` (on load, on open and on every
 * run state), and kept current by the `session_prompt` /
 * `session_prompt_cleared` events. Every update builds a new array.
 *
 * - Answers are in flight per prompt (`promptAnswering` lists their ids): one
 *   stuck answer never disables another node's card. An answer the backend
 *   took but that never clears is released after {@link ANSWER_TIMEOUT_MS}
 *   with a toast; a replaced or cleared prompt releases it at once.
 * - Cleared ids are remembered (`resolvedPrompts`), so a list fetched before
 *   the clear arrived cannot bring the card back; a prompt whose session no
 *   longer waits on it (per the run state) is dropped.
 * - Every prompt is announced once (toast), whichever way it arrived; a
 *   prompt of a run that is not the shown one names that graph and does not
 *   select a node.
 */
import {
  isAppError,
  isSessionPrompt,
  isSessionPromptCleared,
  sessionsOf,
  type Graph,
  type PromptAnswer,
  type RunState,
  type SessionPrompt,
} from '../types/contract';
import type { Api } from '../lib/api';
import { errorMessage } from './runHelpers';
import { notifyUnfocused } from '../lib/notify';

export interface PromptState {
  /** Pending prompts of the shown runs, oldest first. */
  sessionPrompts: readonly SessionPrompt[];
  /** Ids of the prompts whose answer is in flight (their cards are disabled until they clear). */
  promptAnswering: readonly string[];
  /** Ids of prompts known to be answered or gone (most recent last, capped). */
  resolvedPrompts: readonly string[];
  /** Ids of prompts already announced with a toast (capped). */
  announcedPrompts: readonly string[];
}

export interface PromptActions {
  /** Load the prompts of `run` when one of its sessions waits on one we do not know; drop stale ones. */
  syncSessionPrompts: (run: RunState) => Promise<void>;
  /** A `session_prompt` event (validated here). */
  addSessionPrompt: (prompt: unknown) => void;
  /** A `session_prompt_cleared` event. */
  clearSessionPrompt: (cleared: unknown) => void;
  /** Returns true when the backend took the answer. */
  answerPrompt: (prompt: SessionPrompt, answer: PromptAnswer) => Promise<boolean>;
  continueNode: (prompt: SessionPrompt) => Promise<boolean>;
  /** The answers of `nodeId` in `runId` did not land (a stderr event): their cards work again. */
  releaseAnswers: (runId: string, nodeId: string) => boolean;
}

export const INITIAL_PROMPTS: PromptState = {
  sessionPrompts: [],
  promptAnswering: [],
  resolvedPrompts: [],
  announcedPrompts: [],
};

/** An answer the backend took but whose prompt never cleared is released after this long. */
export const ANSWER_TIMEOUT_MS = 30_000;
/** Ids kept in the resolved / announced memories. */
const MEMORY_CAP = 300;
/** A prompt younger than this survives a run state that does not list it yet (event order). */
const RECONCILE_GRACE_MS = 3_000;

type Toast = (kind: 'error' | 'info' | 'warning', text: string, nodeId?: string) => void;
type Get = () => PromptState & { runState: RunState | null; graph: Graph | null };
type SetPatch = (patch: Partial<PromptState>) => void;

const remember = (list: readonly string[], id: string): readonly string[] =>
  list.includes(id) ? list : [...list, id].slice(-MEMORY_CAP);

/** The name the prompt goes by: the node's title when the open graph has it. */
function nodeName(p: SessionPrompt, graph: Graph | null): string {
  const title = graph?.nodes.find((n) => n.id === p.node_id)?.title?.trim();
  return title ? title : p.node_id;
}

/** Toast text of a new prompt. */
export function needsYouText(p: SessionPrompt, graph: Graph | null = null): string {
  const what = p.prompt.type === 'dialog' ? 'a dialog in its session window' : 'a question';
  return `${nodeName(p, graph)} needs you: ${what} — “${p.prompt.question}”`;
}

/** Toast text of a prompt of a run that is not the shown one. */
export function otherRunText(p: SessionPrompt): string {
  return `Another run (${p.run_id}) needs you: node ${p.node_id} — “${p.prompt.question}”`;
}

/**
 * A stderr event of a node whose answer is in flight: the answer did not
 * land (`dialog_answer_failed`, a refusal, an undelivered reply).
 */
export function answerFailed(
  ev: { kind: string; node_id: string; run_id: string },
  prompts: readonly SessionPrompt[],
  answering: readonly string[],
): boolean {
  if (answering.length === 0 || ev.kind !== 'stderr') return false;
  return prompts.some((p) => answering.includes(p.prompt.id) && p.node_id === ev.node_id && p.run_id === ev.run_id);
}

/** The pending prompt of a node in the shown run, or null. */
export function promptOfNode(prompts: readonly SessionPrompt[], run: RunState | null, nodeId: string): SessionPrompt | null {
  if (run === null) return null;
  return prompts.find((p) => p.run_id === run.run_id && p.node_id === nodeId) ?? null;
}

export function createPromptActions(api: Api, set: SetPatch, get: Get, toast: Toast): PromptActions {
  /** When each prompt was added (for the reconcile grace). */
  const addedAt = new Map<string, number>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();

  const stopTimer = (id: string): void => {
    const t = timers.get(id);
    if (t !== undefined) clearTimeout(t);
    timers.delete(id);
  };

  const release = (id: string): void => {
    stopTimer(id);
    const answering = get().promptAnswering;
    if (answering.includes(id)) set({ promptAnswering: answering.filter((x) => x !== id) });
  };

  const without = (id: string): void => {
    stopTimer(id);
    addedAt.delete(id);
    const { sessionPrompts, promptAnswering, resolvedPrompts } = get();
    set({
      sessionPrompts: sessionPrompts.filter((p) => p.prompt.id !== id),
      promptAnswering: promptAnswering.filter((x) => x !== id),
      resolvedPrompts: remember(resolvedPrompts, id),
    });
  };

  const announce = (p: SessionPrompt): void => {
    const { announcedPrompts, runState, graph } = get();
    if (announcedPrompts.includes(p.prompt.id)) return;
    set({ announcedPrompts: remember(announcedPrompts, p.prompt.id) });
    const text = runState === null || runState.run_id === p.run_id ? needsYouText(p, graph) : otherRunText(p);
    if (runState === null || runState.run_id === p.run_id) toast('warning', text, p.node_id);
    else toast('warning', text);
    void notifyUnfocused('Claude Graph', text);
  };

  const add = (p: SessionPrompt): void => {
    const { sessionPrompts: current, resolvedPrompts, promptAnswering } = get();
    if (resolvedPrompts.includes(p.prompt.id)) return;
    const same = current.findIndex((x) => x.prompt.id === p.prompt.id);
    if (same >= 0) {
      // The backend published it again with more detail (the tool behind a dialog).
      set({ sessionPrompts: current.map((x, i) => (i === same ? p : x)) });
      return;
    }
    // One prompt per session: a new one replaces the session's old one.
    const replaced = current.filter((x) => x.run_id === p.run_id && x.sid === p.sid).map((x) => x.prompt.id);
    replaced.forEach((id) => {
      stopTimer(id);
      addedAt.delete(id);
    });
    const others = current.filter((x) => !replaced.includes(x.prompt.id));
    addedAt.set(p.prompt.id, Date.now());
    set({
      sessionPrompts: [...others, p],
      promptAnswering: promptAnswering.filter((id) => !replaced.includes(id)),
    });
    announce(p);
  };

  /** Drop prompts of `run` whose session waits on something else (or nothing, after the grace). */
  const reconcile = (run: RunState): void => {
    const sessions = sessionsOf(run);
    const now = Date.now();
    const stale = get().sessionPrompts.filter((p) => {
      if (p.run_id !== run.run_id) return false;
      const s = sessions.find((x) => x.sid === p.sid);
      if (s === undefined) return false;
      const pending = s.pending_prompt ?? null;
      if (pending === p.prompt.id) return false;
      if (pending !== null) return true;
      return now - (addedAt.get(p.prompt.id) ?? 0) >= RECONCILE_GRACE_MS;
    });
    stale.forEach((p) => without(p.prompt.id));
  };

  const startTimer = (p: SessionPrompt): void => {
    stopTimer(p.prompt.id);
    timers.set(
      p.prompt.id,
      setTimeout(() => {
        timers.delete(p.prompt.id);
        if (!get().promptAnswering.includes(p.prompt.id)) return;
        release(p.prompt.id);
        toast('warning', 'No confirmation from the session window yet; check the window, or answer again.', p.node_id);
      }, ANSWER_TIMEOUT_MS),
    );
  };

  const begin = (p: SessionPrompt): boolean => {
    const answering = get().promptAnswering;
    if (answering.includes(p.prompt.id)) return false;
    set({ promptAnswering: [...answering, p.prompt.id] });
    return true;
  };

  return {
    syncSessionPrompts: async (run) => {
      reconcile(run);
      const { sessionPrompts, resolvedPrompts } = get();
      const known = new Set([...sessionPrompts.map((p) => p.prompt.id), ...resolvedPrompts]);
      const ids = sessionsOf(run)
        .map((s) => s.pending_prompt)
        .filter((id): id is string => typeof id === 'string');
      if (ids.every((id) => known.has(id))) return;
      try {
        const listed = await api.listSessionPrompts(run.run_id);
        listed.forEach((p) => {
          if (isSessionPrompt(p)) add(p);
          else console.warn('ignored a malformed session prompt from list_session_prompts', p);
        });
      } catch (err) {
        if (!(isAppError(err) && err.code === 'not_found')) toast('error', `Could not load the questions of the run: ${errorMessage(err)}`);
      }
    },
    addSessionPrompt: (prompt) => {
      if (!isSessionPrompt(prompt)) {
        console.warn('ignored a malformed session_prompt payload', prompt);
        return;
      }
      add(prompt);
    },
    clearSessionPrompt: (cleared) => {
      if (isSessionPromptCleared(cleared)) without(cleared.id);
    },
    answerPrompt: async (p, answer) => {
      if (!begin(p)) return false;
      try {
        await api.answerPrompt(p.run_id, p.sid, p.prompt.id, answer);
        if (get().promptAnswering.includes(p.prompt.id)) startTimer(p);
        return true;
      } catch (err) {
        release(p.prompt.id);
        if (isAppError(err) && err.code === 'not_found') {
          without(p.prompt.id);
          toast('info', 'That question was already answered.');
          return false;
        }
        toast('error', `Could not send your answer: ${errorMessage(err)}`, p.node_id);
        return false;
      }
    },
    continueNode: async (p) => {
      if (!begin(p)) return false;
      try {
        await api.continueNode(p.run_id, p.sid, p.node_id);
        if (get().promptAnswering.includes(p.prompt.id)) startTimer(p);
        return true;
      } catch (err) {
        release(p.prompt.id);
        // The question is gone (answered in the window, or the node moved on).
        if (isAppError(err) && (err.code === 'not_found' || err.code === 'invalid_input')) {
          without(p.prompt.id);
          toast('info', 'That question is no longer waiting.');
          return false;
        }
        toast('error', `Could not continue: ${errorMessage(err)}`, p.node_id);
        return false;
      }
    },
    releaseAnswers: (runId, nodeId) => {
      const ids = get()
        .sessionPrompts.filter((p) => p.run_id === runId && p.node_id === nodeId)
        .map((p) => p.prompt.id)
        .filter((id) => get().promptAnswering.includes(id));
      ids.forEach(release);
      return ids.length > 0;
    },
  };
}

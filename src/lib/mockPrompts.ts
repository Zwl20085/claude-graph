/**
 * Turn outcomes in the mock backend (TURN_CONTRACT): pending session prompts
 * (a dialog or a reply question), `answer_prompt`, `continue_node`,
 * `list_session_prompts`, and the `session_prompt` / `session_prompt_cleared`
 * events. The mock runner opens a prompt and resumes the node when it is
 * answered (see `createMockRunner`'s `prompts`).
 */
import type {
  AppError,
  NodeEventKind,
  PromptAnswer,
  SessionPrompt,
  SessionPromptCleared,
  TurnPrompt,
} from '../types/contract';
import { promptLabels, promptTakesText } from '../types/contract';

/** What the user decided: an answer, or "continue without answering". */
export type MockDecision = PromptAnswer | 'continue';

export interface MockPromptHooks {
  /** Node `nodeId` (session `sid`) of `runId` waits on `prompt`; `resume` is called once with the decision. */
  open: (runId: string, sid: string, nodeId: string, prompt: TurnPrompt, resume: (d: MockDecision) => void) => void;
  /** Id of the prompt session `sid` waits on, or null. */
  pendingOf: (runId: string, sid: string) => string | null;
  /** Drop the prompts of a run (of one node), without resuming. */
  cancel: (runId: string, nodeId?: string) => void;
}

export interface MockPromptDeps {
  emitPrompt: (p: SessionPrompt) => void;
  emitCleared: (c: SessionPromptCleared) => void;
  emitEvent: (runId: string, nodeId: string, kind: NodeEventKind, text: string) => void;
}

const appError = (code: string, message: string): AppError => ({ code, message });

/** The label an answer picks (or its text). */
export function answerText(p: TurnPrompt, a: PromptAnswer): string {
  const labels = promptLabels(p);
  if ('text' in a) return a.text;
  if ('choice' in a) return labels[a.choice]?.label ?? '';
  return a.choices.map((i) => labels[i]?.label ?? '').join(', ');
}

/** Backend rule of `answer_prompt` (prompt_api.rs `check_answer`). */
export function checkAnswer(p: TurnPrompt, a: PromptAnswer): AppError | null {
  const count = promptLabels(p).length;
  const multi = p.type === 'dialog' && p.multi;
  if ('choice' in a) return a.choice >= 0 && a.choice < count ? null : appError('invalid_input', `no option ${a.choice}`);
  if ('choices' in a) {
    if (!multi) return appError('invalid_input', 'this prompt takes one option, not several');
    return a.choices.every((i) => i >= 0 && i < count) ? null : appError('invalid_input', 'no such option');
  }
  if (!promptTakesText(p)) return appError('invalid_input', 'this prompt only takes one of its options');
  return a.text.trim() === '' ? appError('invalid_input', 'the answer is empty') : null;
}

interface Entry {
  sp: SessionPrompt;
  resume: (d: MockDecision) => void;
}

export function createMockPrompts(deps: MockPromptDeps) {
  let pending: readonly Entry[] = [];

  const take = (match: (e: Entry) => boolean): Entry | null => {
    const found = pending.find(match) ?? null;
    if (found) pending = pending.filter((e) => e !== found);
    return found;
  };

  const clear = (e: Entry, via: SessionPromptCleared['via'], text: string): void => {
    const { run_id, sid, node_id, prompt } = e.sp;
    deps.emitCleared({ run_id, sid, node_id, id: prompt.id, via });
    deps.emitEvent(run_id, node_id, 'answer', text);
  };

  const hooks: MockPromptHooks = {
    open: (runId, sid, nodeId, prompt, resume) => {
      const sp: SessionPrompt = { run_id: runId, sid, node_id: nodeId, prompt };
      pending = [...pending, { sp, resume }];
      const labels = promptLabels(prompt).map((c, i) => `${i + 1}. ${c.label}`);
      deps.emitEvent(runId, nodeId, prompt.type === 'dialog' ? 'dialog' : 'question', [prompt.question, ...labels].join(' / '));
      deps.emitPrompt(sp);
    },
    pendingOf: (runId, sid) => pending.find((e) => e.sp.run_id === runId && e.sp.sid === sid)?.sp.prompt.id ?? null,
    cancel: (runId, nodeId) => {
      pending = pending.filter((e) => !(e.sp.run_id === runId && (nodeId === undefined || e.sp.node_id === nodeId)));
    },
  };

  return {
    hooks,
    list: (runId: string): SessionPrompt[] => pending.filter((e) => e.sp.run_id === runId).map((e) => e.sp),
    answer(runId: string, sid: string, promptId: string, answer: PromptAnswer): void {
      const e = pending.find((x) => x.sp.run_id === runId && x.sp.sid === sid && x.sp.prompt.id === promptId);
      if (!e) throw appError('not_found', `prompt ${promptId} is no longer waiting (answered or gone)`);
      const bad = checkAnswer(e.sp.prompt, answer);
      if (bad) throw bad;
      take((x) => x === e);
      clear(e, 'app', `answered from the app: ${answerText(e.sp.prompt, answer)}`);
      e.resume(answer);
    },
    continueNode(runId: string, sid: string, nodeId: string): void {
      const e = take((x) => x.sp.run_id === runId && x.sp.sid === sid && x.sp.node_id === nodeId && x.sp.prompt.type === 'reply');
      if (!e) throw appError('invalid_input', `node ${nodeId} is not waiting on a question`);
      clear(e, 'continue', 'continued without answering');
      e.resume('continue');
    },
  };
}

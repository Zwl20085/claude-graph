/**
 * Turn outcomes in terminal mode (TURN_CONTRACT; Rust `contract_prompt.rs`):
 * the question or dialog a session waits on, its answer, and the records of
 * runs/<run>/<node>/questions.jsonl. Re-exported by `contract.ts`.
 */

/** Event: a `SessionPrompt` (a node waits for the user). */
export const EVENT_SESSION_PROMPT = 'session_prompt';
/** Event: a `SessionPromptCleared` (answered, continued, or the dialog went away). */
export const EVENT_SESSION_PROMPT_CLEARED = 'session_prompt_cleared';

export const DIALOG_KINDS = ['choice', 'permission', 'text', 'unknown'] as const;
export type DialogKind = (typeof DIALOG_KINDS)[number];

export const PROMPT_SOURCES = ['transcript', 'screen', 'reply', 'events'] as const;
export type PromptSource = (typeof PROMPT_SOURCES)[number];

export const ANSWER_VIAS = ['app', 'window', 'continue'] as const;
export type AnswerVia = (typeof ANSWER_VIAS)[number];

export interface PromptOption {
  /** 0-based position in the list. */
  index: number;
  label: string;
  detail?: string;
}

/** An interactive prompt on screen while the turn is still running. */
export interface DialogPrompt {
  type: 'dialog';
  id: string;
  kind: DialogKind;
  question: string;
  header?: string;
  options: PromptOption[];
  /** Highlighted option, when known. */
  selected?: number;
  multi: boolean;
  allows_text: boolean;
  source: PromptSource;
}

export interface ReplyChoice {
  label: string;
  detail?: string;
}

/** The turn ended and its reply asks the user something. */
export interface ReplyQuestion {
  type: 'reply';
  id: string;
  question: string;
  choices: ReplyChoice[];
  /** The end of the reply (at most 1200 chars). */
  reply_excerpt: string;
}

export type TurnPrompt = DialogPrompt | ReplyQuestion;

/** Payload of `session_prompt`; entry of `list_session_prompts`. */
export interface SessionPrompt {
  run_id: string;
  sid: string;
  node_id: string;
  prompt: TurnPrompt;
}

/** Payload of `session_prompt_cleared`. */
export interface SessionPromptCleared {
  run_id: string;
  sid: string;
  node_id: string;
  id: string;
  via: AnswerVia;
}

/** `answer` of `answer_prompt`. */
export type PromptAnswer = { choice: number } | { choices: number[] } | { text: string };

export interface QuestionAnswer {
  ts: string;
  via: AnswerVia;
  text?: string;
  index?: number;
}

/** One line of questions.jsonl (`NodeResult.questions`). */
export interface QuestionRecord {
  id: string;
  ts: string;
  kind: 'question' | 'dialog';
  source: PromptSource;
  question: string;
  header?: string;
  options: ReplyChoice[];
  selected?: number;
  /** A reply question that did not pause the node (its deliverables were written). */
  offer?: boolean;
  answer?: QuestionAnswer;
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const optString = (v: unknown): boolean => v === undefined || v === null || typeof v === 'string';
const isIndex = (v: unknown): v is number => typeof v === 'number' && Number.isInteger(v) && v >= 0;

function isOption(v: unknown): v is PromptOption {
  return isObject(v) && isIndex(v.index) && typeof v.label === 'string' && optString(v.detail);
}

function isChoice(v: unknown): v is ReplyChoice {
  return isObject(v) && typeof v.label === 'string' && optString(v.detail);
}

export function isTurnPrompt(v: unknown): v is TurnPrompt {
  if (!isObject(v) || typeof v.id !== 'string' || v.id === '' || typeof v.question !== 'string') return false;
  if (v.type === 'reply') {
    return Array.isArray(v.choices) && v.choices.every(isChoice) && typeof v.reply_excerpt === 'string';
  }
  return (
    v.type === 'dialog' &&
    (DIALOG_KINDS as readonly unknown[]).includes(v.kind) &&
    Array.isArray(v.options) &&
    v.options.every(isOption) &&
    (v.selected === undefined || v.selected === null || isIndex(v.selected)) &&
    typeof v.multi === 'boolean' &&
    typeof v.allows_text === 'boolean' &&
    (PROMPT_SOURCES as readonly unknown[]).includes(v.source) &&
    optString(v.header)
  );
}

export function isSessionPrompt(v: unknown): v is SessionPrompt {
  return (
    isObject(v) &&
    typeof v.run_id === 'string' &&
    typeof v.sid === 'string' &&
    typeof v.node_id === 'string' &&
    isTurnPrompt(v.prompt)
  );
}

export function isSessionPromptCleared(v: unknown): v is SessionPromptCleared {
  return (
    isObject(v) &&
    typeof v.run_id === 'string' &&
    typeof v.sid === 'string' &&
    typeof v.node_id === 'string' &&
    typeof v.id === 'string' &&
    (ANSWER_VIAS as readonly unknown[]).includes(v.via)
  );
}

export function isQuestionRecord(v: unknown): v is QuestionRecord {
  return (
    isObject(v) &&
    typeof v.id === 'string' &&
    typeof v.ts === 'string' &&
    (v.kind === 'question' || v.kind === 'dialog') &&
    typeof v.question === 'string' &&
    Array.isArray(v.options) &&
    v.options.every(isChoice)
  );
}

/** Well-formed records of a `NodeResult.questions` (missing = []). */
export function questionsOf(result: { questions?: unknown } | null | undefined): QuestionRecord[] {
  const v = result?.questions;
  return Array.isArray(v) ? v.filter(isQuestionRecord) : [];
}

/** Labels a prompt offers as buttons (dialog options or reply choices). */
export function promptLabels(p: TurnPrompt): ReplyChoice[] {
  return p.type === 'dialog' ? p.options.map((o) => ({ label: o.label, detail: o.detail })) : p.choices;
}

/** A free-text answer is possible. */
export function promptTakesText(p: TurnPrompt): boolean {
  return p.type === 'reply' || p.allows_text || p.kind === 'text';
}

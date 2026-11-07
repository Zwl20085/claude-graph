/** Pure presentation rules of the Assistant tab (labels and colours per status / kind). */
import type { AssistantKind, AssistantStatus } from '../types/contract';

export interface StatusLook {
  label: string;
  color: string;
  pulse: boolean;
}

export const STATUS_LOOK: Readonly<Record<AssistantStatus, StatusLook>> = {
  idle: { label: 'Idle', color: '#8a8a8a', pulse: false },
  working: { label: 'Working…', color: '#3b82f6', pulse: true },
  waiting: { label: 'Waiting for your answer', color: '#f59e0b', pulse: false },
  done: { label: 'Done', color: '#22c55e', pulse: false },
  failed: { label: 'Failed', color: '#ef4444', pulse: false },
  cancelled: { label: 'Cancelled', color: '#8a8a8a', pulse: false },
};

export const KIND_COLOR: Readonly<Record<AssistantKind, string>> = {
  instruction: '#3b82f6',
  answer: '#3b82f6',
  stage: '#22c55e',
  question: '#f59e0b',
  decision: '#60a5fa',
  note: '#8a8a8a',
  final: '#a855f7',
  system: '#6b7280',
  error: '#ef4444',
};

const KIND_LABEL: Readonly<Record<AssistantKind, string>> = {
  instruction: 'Instruction',
  answer: 'Answer',
  stage: 'Stage',
  question: 'Question',
  decision: 'Decision',
  note: 'Note',
  final: 'Final',
  system: 'System',
  error: 'Error',
};

/** Card heading: "Stage: survey", "Decision: <title>", "Question", … */
export function cardHeading(kind: AssistantKind, title: string | null): string {
  const label = KIND_LABEL[kind];
  return title !== null && title.trim() !== '' ? `${label}: ${title.trim()}` : label;
}

/** TopBar pill: only while the scheduler works or waits for the user. */
export function showsPill(status: AssistantStatus | null | undefined): boolean {
  return status === 'working' || status === 'waiting';
}

export const COMPOSER_PLACEHOLDER = {
  instruction: 'Give the scheduler an instruction…',
  answer: 'Answer the question…',
} as const;

export function composerPlaceholder(status: AssistantStatus | null | undefined): string {
  return status === 'waiting' ? COMPOSER_PLACEHOLDER.answer : COMPOSER_PLACEHOLDER.instruction;
}

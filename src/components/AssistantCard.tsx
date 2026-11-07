/** One journal entry of the assistant timeline: a card with a coloured left bar and an icon per kind. */
import { CircleCheck, CircleHelp, Flag, Info, Scale, StickyNote, TriangleAlert, User, type LucideIcon } from 'lucide-react';
import type { AssistantKind, AssistantMessage } from '../types/contract';
import { cardHeading, KIND_COLOR } from '../lib/assistantView';
import { btnCls } from './ui';

const ICONS: Readonly<Record<AssistantKind, LucideIcon>> = {
  instruction: User,
  answer: User,
  stage: CircleCheck,
  question: CircleHelp,
  decision: Scale,
  note: StickyNote,
  final: Flag,
  system: Info,
  error: TriangleAlert,
};

interface AssistantCardProps {
  message: AssistantMessage;
  /** Shown on question cards: focuses the composer. */
  onAnswer?: () => void;
}

export function AssistantCard({ message, onAnswer }: AssistantCardProps) {
  const { kind, title, text } = message;
  const Icon = ICONS[kind];
  const color = KIND_COLOR[kind];
  const user = message.role === 'user';
  const system = kind === 'system';
  const heading = cardHeading(kind, title);
  const tone = user ? 'bg-[#1e3a5f] text-[#e5e5e5]' : system ? 'bg-transparent text-[#8a8a8a]' : 'bg-[#1e1e1e] text-[#d4d4d4]';
  return (
    <div className={`flex ${user ? 'justify-end' : 'justify-start'}`}>
      <article
        data-kind={kind}
        aria-label={heading}
        className={`max-w-[92%] rounded border-l-[3px] px-2 py-1.5 text-[12px] ${tone}`}
        style={{ borderLeftColor: color }}
      >
        <header className="mb-0.5 flex items-center gap-1 text-[11px] font-semibold" style={{ color }}>
          <Icon size={12} aria-hidden="true" />
          <span>{heading}</span>
          <span className="ml-auto pl-2 font-normal text-[#6b7280]">turn {message.turn}</span>
        </header>
        <div className={`whitespace-pre-wrap break-words ${system ? 'italic' : ''}`}>{text}</div>
        {kind === 'question' && onAnswer && (
          <button className={`${btnCls} mt-1 border border-[#f59e0b]/50 text-[#f59e0b]`} onClick={onAnswer} aria-label="answer this question">
            Answer
          </button>
        )}
      </article>
    </div>
  );
}

/**
 * "Needs you" card (TURN_CONTRACT): the question or dialog a node of the
 * shown terminal run waits on. Option buttons (detail as tooltip), checkboxes
 * plus Confirm for a multi-select dialog, a free-text composer where the
 * prompt takes text, "Continue without answering" for a reply question, and
 * "Open window". A pressed button disables the card until the prompt clears.
 */
import { useState } from 'react';
import { CircleHelp, Send, SquareTerminal } from 'lucide-react';
import { promptLabels, promptTakesText, type PromptAnswer, type SessionPrompt } from '../types/contract';
import { STATUS_COLORS } from '../lib/status';
import { useGraphStore } from '../store/graphStore';
import { openSessionWindow } from './SessionChips';
import { btnCls, inputCls } from './ui';

export const CONTINUE_LABEL = 'Continue without answering';
export const CONTINUE_HINT = 'Treat the question as an offer: the node is done (status by its deliverables) and the next node starts.';
const KIND_TEXT: Record<string, string> = {
  choice: 'asks you to choose',
  permission: 'asks for permission',
  text: 'asks for an answer',
  unknown: 'waits for you',
};

interface Props {
  prompt: SessionPrompt;
}

export function PromptCard({ prompt }: Props) {
  const answering = useGraphStore((s) => s.promptAnswering);
  const answer = useGraphStore((s) => s.answerPrompt);
  const continueNode = useGraphStore((s) => s.continueNode);
  const pushToast = useGraphStore((s) => s.pushToast);
  const [text, setText] = useState('');
  const [picked, setPicked] = useState<readonly number[]>([]);
  const p = prompt.prompt;
  const busy = answering !== null;
  const labels = promptLabels(p);
  const multi = p.type === 'dialog' && p.multi;
  const color = STATUS_COLORS.waiting;
  const send = (a: PromptAnswer): void => void answer(prompt, a);
  const lead = p.type === 'dialog' ? `A dialog in the window ${KIND_TEXT[p.kind] ?? KIND_TEXT.unknown}` : 'The reply ends with a question';

  return (
    <section
      aria-label="needs you"
      aria-busy={busy}
      className="space-y-2 rounded border px-2 py-2 text-[12px]"
      style={{ borderColor: color }}
    >
      <p className="flex items-center gap-1 font-semibold" style={{ color }}>
        <CircleHelp size={13} />Needs you · <span className="font-normal">{lead}</span>
      </p>
      {p.type === 'dialog' && p.header && <p className="text-[11px] text-[#9a9a9a]">{p.header}</p>}
      <p className="whitespace-pre-wrap text-[#e5e5e5]">{p.question || '(no question text)'}</p>
      {labels.length > 0 && (
        <div className="flex flex-col gap-1" role="group" aria-label="options">
          {labels.map((c, i) =>
            multi ? (
              <label key={`${i}-${c.label}`} className="flex items-center gap-1.5" title={c.detail}>
                <input
                  type="checkbox"
                  disabled={busy}
                  checked={picked.includes(i)}
                  onChange={(e) => setPicked(e.target.checked ? [...picked, i] : picked.filter((x) => x !== i))}
                />
                {c.label}
              </label>
            ) : (
              <button
                key={`${i}-${c.label}`}
                className={`${btnCls} justify-start border border-[#3c3c3c] text-left`}
                disabled={busy}
                title={c.detail}
                onClick={() => send({ choice: i })}
              >
                <span className="font-mono text-[#9a9a9a]">{i + 1}.</span> {c.label}
              </button>
            ),
          )}
          {multi && (
            <button
              className={`${btnCls} self-start bg-[#3b82f6] text-white`}
              disabled={busy || picked.length === 0}
              onClick={() => send({ choices: [...picked].sort((a, b) => a - b) })}
            >
              Confirm
            </button>
          )}
        </div>
      )}
      {promptTakesText(p) && (
        <div className="flex gap-1">
          <textarea
            aria-label="your answer"
            className={`${inputCls} resize-none`}
            rows={2}
            disabled={busy}
            value={text}
            placeholder={p.type === 'reply' ? 'Answer (sent to the session as your next message)' : 'Your own answer'}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && text.trim() !== '') {
                e.preventDefault();
                send({ text: text.trim() });
              }
            }}
          />
          <button
            className={`${btnCls} self-end bg-[#3b82f6] text-white`}
            disabled={busy || text.trim() === ''}
            onClick={() => send({ text: text.trim() })}
          >
            <Send size={13} />Answer
          </button>
        </div>
      )}
      <div className="flex flex-wrap gap-1">
        {p.type === 'reply' && (
          <button
            className={`${btnCls} border border-[#3c3c3c]`}
            disabled={busy}
            title={CONTINUE_HINT}
            onClick={() => void continueNode(prompt)}
          >
            {CONTINUE_LABEL}
          </button>
        )}
        <button
          className={`${btnCls} border border-[#3c3c3c]`}
          onClick={() => openSessionWindow(prompt.run_id, prompt.sid, (t) => pushToast('error', t))}
        >
          <SquareTerminal size={13} />Open window
        </button>
      </div>
      {busy && <p className="text-[11px] text-[#9a9a9a]" role="status">Sending…</p>}
    </section>
  );
}

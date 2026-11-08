/**
 * Messages tab of a terminal-mode run: the text is typed into the node's
 * Claude Code session (`pty_write` with "\r") while that session is alive;
 * the conversation itself lives in the session window.
 */
import { useEffect, useRef, useState } from 'react';
import { Send, SquareTerminal } from 'lucide-react';
import { sessionsOf } from '../types/contract';
import { api } from '../lib/api';
import { isSessionAlive, sessionOfNode } from '../lib/sessions';
import { useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';
import { promptOfNode } from '../store/promptSlice';
import { PromptCard } from './PromptCard';
import { openSessionWindow } from './SessionChips';
import { btnCls, inputCls } from './ui';

export const SESSION_CLOSED_TEXT = 'Session closed; rerun to talk';
/** Composer hint while a dialog that takes no typed answer is open. */
export const OPTIONS_ONLY_TEXT = 'this prompt takes one of its options';

/** One line for the TUI: a raw newline would submit early, so line breaks become spaces. */
export function terminalLine(text: string): string {
  return text.trim().replace(/\s*\r?\n\s*/g, ' ');
}

export function TerminalMessages({ nodeId }: { nodeId: string }) {
  const runState = useGraphStore((s) => s.runState);
  const pushToast = useGraphStore((s) => s.pushToast);
  const pending = useGraphStore((s) => promptOfNode(s.sessionPrompts, s.runState, nodeId));
  const [draft, setDraft] = useState('');
  const [sent, setSent] = useState<readonly string[]>([]);
  const session = sessionOfNode(sessionsOf(runState), nodeId);
  const runId = runState?.run_id ?? null;
  const alive = runId !== null && isSessionAlive(session);
  const hint = alive && session ? `Typed into the Claude Code window “${session.title || session.sid}”.` : SESSION_CLOSED_TEXT;
  const toast = (text: string): void => pushToast('error', text);

  const answerPrompt = useGraphStore((s) => s.answerPrompt);
  const dialog = pending !== null && pending.prompt.type === 'dialog' ? pending.prompt : null;
  // Keys typed into an open dialog would pick its highlighted option: only a
  // dialog with a free-text option takes the composer's text (as its answer).
  const blocked = dialog !== null && !dialog.allows_text;
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const pendingId = pending?.prompt.id ?? null;
  const hadPending = useRef(pendingId);
  useEffect(() => {
    // The card went away (answered): keep the keyboard in the Messages tab.
    if (hadPending.current !== null && pendingId === null) composerRef.current?.focus();
    hadPending.current = pendingId;
  }, [pendingId]);

  const send = async (): Promise<void> => {
    const line = terminalLine(draft);
    if (line === '' || !alive || !session || runId === null || blocked) return;
    setDraft('');
    if (dialog !== null && pending !== null) {
      if (!(await answerPrompt(pending, { text: line }))) setDraft(line);
      return;
    }
    try {
      await api.ptyWrite(runId, session.sid, `${line}\r`);
      setSent((list) => [...list, line]);
    } catch (err) {
      setDraft(line);
      toast(`Message not sent: ${errorMessage(err)}`);
    }
  };

  return (
    <div className="flex h-full flex-col gap-2">
      {pending && <PromptCard key={pending.prompt.id} prompt={pending} />}
      <div className="min-h-0 flex-1 space-y-2 overflow-auto">
        {sent.length === 0 && <p className="text-[12px] text-[#777]">The conversation is in the session window.</p>}
        {sent.map((text, i) => (
          <div key={`${i}-${text}`} className="flex justify-end">
            <div className="max-w-[90%] whitespace-pre-wrap break-words rounded bg-[#1e3a5f] px-2 py-1.5 text-[12px] text-[#e5e5e5]">{text}</div>
          </div>
        ))}
      </div>
      <p className="text-[11px] text-[#9a9a9a]" role="status">
        {blocked ? `A dialog is open: ${OPTIONS_ONLY_TEXT}.` : hint}
      </p>
      {alive && session && runId !== null && (
        <button
          className={`${btnCls} self-start border border-[#3c3c3c]`}
          onClick={() => openSessionWindow(runId, session.sid, toast)}
          aria-label="open the Claude Code window of this node"
        >
          <SquareTerminal size={13} />Open window
        </button>
      )}
      <div className="flex gap-1">
        <textarea
          aria-label="message to this node"
          className={`${inputCls} resize-none`}
          rows={3}
          ref={composerRef}
          disabled={!alive || blocked}
          value={draft}
          placeholder={blocked ? OPTIONS_ONLY_TEXT : alive ? 'Type into the session (Ctrl+Enter to send)' : SESSION_CLOSED_TEXT}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button className={`${btnCls} self-end bg-[#3b82f6] text-white`} disabled={!alive || blocked || draft.trim() === ''} onClick={() => void send()}>
          <Send size={13} />Send
        </button>
      </div>
    </div>
  );
}

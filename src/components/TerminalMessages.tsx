/**
 * Chat tab of a terminal-mode run: the text is typed into the node's session
 * (`pty_write` with "\r") while that session takes messages. Above the
 * composer: the node's last reply (its reply.md / result.md) and the lines
 * sent from here in this run (kept in the store). While a dialog without a
 * text option is open the composer is disabled; a closed session offers
 * "Run from here" instead.
 */
import { useEffect, useRef, useState } from 'react';
import { Play, Send, SquareTerminal } from 'lucide-react';
import { sessionsOf } from '../types/contract';
import { api } from '../lib/api';
import { Markdown } from '../lib/markdown';
import { isSessionAlive, sessionOfNode } from '../lib/sessions';
import { useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';
import { promptOfNode } from '../store/promptSlice';
import { sentKey } from '../store/uiSlice';
import { PromptCard } from './PromptCard';
import { openSessionWindow } from './SessionChips';
import { btnCls, inputCls } from './ui';

export const SESSION_CLOSED_TEXT = 'This node’s session has closed.';
/** Composer hint while a dialog that takes no typed answer is open. */
export const OPTIONS_ONLY_TEXT = 'this prompt takes one of its options';
export const ONE_LINE_NOTE = 'Line breaks are sent as spaces: the message goes as one line.';
const NO_EMPTY: readonly string[] = [];

/** One line for the TUI: a raw newline would submit early, so line breaks become spaces. */
export function terminalLine(text: string): string {
  return text.trim().replace(/\s*\r?\n\s*/g, ' ');
}

/** The node's reply in `runId` (result.md), reloaded when the node's status changes. */
function useLastReply(runId: string | null, nodeId: string): string | null {
  const dir = useGraphStore((s) => s.dir);
  const status = useGraphStore((s) => s.runState?.nodes[nodeId]?.status ?? null);
  const [reply, setReply] = useState<string | null>(null);
  useEffect(() => {
    if (!dir || runId === null) return setReply(null);
    let live = true;
    api
      .readNodeResult(dir, runId, nodeId)
      .then((r) => live && setReply(r.result_md))
      .catch(() => live && setReply(null));
    return () => {
      live = false;
    };
  }, [dir, runId, nodeId, status]);
  return reply;
}

export function TerminalMessages({ nodeId }: { nodeId: string }) {
  const runState = useGraphStore((s) => s.runState);
  const pushToast = useGraphStore((s) => s.pushToast);
  const pending = useGraphStore((s) => promptOfNode(s.sessionPrompts, s.runState, nodeId));
  const runId = runState?.run_id ?? null;
  const sent = useGraphStore((s) => (runId === null ? NO_EMPTY : (s.sentLines[sentKey(runId, nodeId)] ?? NO_EMPTY)));
  const addSentLine = useGraphStore((s) => s.addSentLine);
  const startRun = useGraphStore((s) => s.startRun);
  const answerPrompt = useGraphStore((s) => s.answerPrompt);
  const [draft, setDraft] = useState('');
  const session = sessionOfNode(sessionsOf(runState), nodeId);
  const alive = runId !== null && isSessionAlive(session);
  const reply = useLastReply(runId, nodeId);
  const toast = (text: string): void => pushToast('error', text, nodeId);

  const dialog = pending !== null && pending.prompt.type === 'dialog' ? pending.prompt : null;
  // Keys typed into an open dialog would pick its highlighted option: only a
  // dialog with a free-text option takes the composer's text (as its answer).
  const blocked = dialog !== null && !dialog.allows_text;
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const pendingId = pending?.prompt.id ?? null;
  const hadPending = useRef(pendingId);
  useEffect(() => {
    // The card went away (answered): keep the keyboard in the Chat tab.
    if (hadPending.current !== null && pendingId === null) composerRef.current?.focus();
    hadPending.current = pendingId;
  }, [pendingId]);

  const deliver = async (line: string): Promise<void> => {
    if (!alive || !session || runId === null) return;
    try {
      await api.ptyWrite(runId, session.sid, `${line}\r`);
      addSentLine(runId, nodeId, line);
    } catch (err) {
      setDraft(line);
      pushToast('error', `Message to ${nodeId} not sent: ${errorMessage(err)}`, nodeId, {
        label: 'Retry',
        run: () => void deliver(line),
      });
    }
  };

  const send = async (): Promise<void> => {
    const line = terminalLine(draft);
    if (line === '' || !alive || !session || runId === null || blocked) return;
    setDraft('');
    if (dialog !== null && pending !== null) {
      if (!(await answerPrompt(pending, { text: line }))) setDraft(line);
      return;
    }
    await deliver(line);
  };

  const hint = blocked
    ? `A dialog is open: ${OPTIONS_ONLY_TEXT}.`
    : alive && session
      ? `Typed into the session window “${session.title || session.sid}”.`
      : SESSION_CLOSED_TEXT;

  return (
    <div className="flex h-full flex-col gap-2">
      {pending && <PromptCard key={pending.prompt.id} prompt={pending} />}
      <div className="min-h-0 flex-1 space-y-2 overflow-auto">
        {reply && (
          <section aria-label="last reply" className="rounded border border-[#3c3c3c] p-2">
            <p className="mb-1 text-[11px] uppercase tracking-wide text-[#9a9a9a]">Last reply</p>
            <Markdown text={reply} />
          </section>
        )}
        {sent.length === 0 && !reply && <p className="text-[12px] text-[#777]">The conversation is in the session window.</p>}
        {sent.map((text, i) => (
          <div key={`${i}-${text}`} className="flex justify-end">
            <div className="max-w-[90%] whitespace-pre-wrap break-words rounded bg-[#1e3a5f] px-2 py-1.5 text-[12px] text-[#e5e5e5]">{text}</div>
          </div>
        ))}
      </div>
      <p className="text-[11px] text-[#9a9a9a]" role="status">
        {hint}
      </p>
      {alive && session && runId !== null && (
        <button
          className={`${btnCls} self-start border border-[#3c3c3c]`}
          onClick={() => openSessionWindow(runId, session.sid, toast)}
          aria-label="open the session window of this node"
        >
          <SquareTerminal size={13} />Open window
        </button>
      )}
      {!alive && runId !== null && session ? (
        <button className={`${btnCls} self-start border border-[#3b82f6] text-white`} onClick={() => void startRun({ from: nodeId })}>
          <Play size={13} />Run from here
        </button>
      ) : (
        <>
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
            <button
              className={`${btnCls} self-end bg-[#3b82f6] text-white`}
              title="Send (Ctrl+Enter)"
              disabled={!alive || blocked || draft.trim() === ''}
              onClick={() => void send()}
            >
              <Send size={13} />Send
            </button>
          </div>
          {/\n/.test(draft.trim()) && <p className="text-[11px] text-[#f59e0b]">{ONE_LINE_NOTE}</p>}
        </>
      )}
    </div>
  );
}

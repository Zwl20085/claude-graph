/**
 * "Ask me in the app": one pending permission prompt at a time, oldest first
 * (ASKAPP_CONTRACT). Enter = Allow once, Esc = Deny (with the optional
 * message), Tab stays inside, focus returns where it was when the queue empties.
 */
import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react';
import { ShieldQuestion } from 'lucide-react';
import { ASSISTANT_RUN_ID, type PermissionAnswer, type PermissionPrompt, type PermissionScope } from '../types/contract';
import { useGraphStore } from '../store/graphStore';
import { btnCls, FOCUSABLE, inputCls, trapTab } from './ui';

/** Pretty JSON of the raw tool input (falls back to String for unserialisable values). */
export function prettyInput(input: unknown): string {
  try {
    return JSON.stringify(input, null, 2) ?? String(input);
  } catch {
    return String(input);
  }
}

export function allowAnswer(id: string, scope: PermissionScope): PermissionAnswer {
  return { id, allow: true, scope };
}

export function denyAnswer(id: string, message: string): PermissionAnswer {
  const text = message.trim();
  return text === '' ? { id, allow: false, scope: 'once' } : { id, allow: false, scope: 'once', message: text };
}

/**
 * Allow (by Enter or by the Allow buttons) is ignored for this long after a
 * prompt becomes the shown one, so a held or repeated Enter cannot approve
 * the next queued prompt by accident.
 */
export const ARM_DELAY_MS = 500;

/** Elements on which Enter keeps its native meaning (activate, toggle, newline). */
const NATIVE_ENTER = new Set(['BUTTON', 'SUMMARY', 'TEXTAREA', 'A', 'SELECT']);

export interface PermissionDialogViewProps {
  prompt: PermissionPrompt;
  /** 0-based position of `prompt` in the queue. */
  index: number;
  total: number;
  /** Title of the asking node; null when unknown (shows the id only). */
  nodeTitle: string | null;
  busy: boolean;
  /** Allow is ignored this long after mount (default ARM_DELAY_MS; 0 = at once). */
  armDelayMs?: number;
  onAnswer: (answer: PermissionAnswer) => void;
  onHide: () => void;
}

export function PermissionDialogView({
  prompt,
  index,
  total,
  nodeTitle,
  busy,
  armDelayMs = ARM_DELAY_MS,
  onAnswer,
  onHide,
}: PermissionDialogViewProps) {
  const titleId = useId();
  const descId = useId();
  const messageId = useId();
  const box = useRef<HTMLDivElement>(null);
  const [message, setMessage] = useState('');
  const [armed, setArmed] = useState(armDelayMs <= 0);

  useEffect(() => {
    if (armDelayMs <= 0) return;
    const handle = setTimeout(() => setArmed(true), armDelayMs);
    return () => clearTimeout(handle);
  }, [armDelayMs]);

  useEffect(() => {
    box.current?.querySelector<HTMLElement>('[data-autofocus]')?.focus();
  }, []);

  const allow = (scope: PermissionScope): void => {
    if (!busy && armed) onAnswer(allowAnswer(prompt.id, scope));
  };
  const deny = (): void => {
    if (!busy) onAnswer(denyAnswer(prompt.id, message));
  };

  const unarmedCls = armed ? '' : 'cursor-not-allowed opacity-40';

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>): void => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      deny();
    } else if (e.key === 'Enter' && !e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey) {
      const target = e.target as HTMLElement;
      if (NATIVE_ENTER.has(target.tagName)) return;
      e.preventDefault();
      // Enter in the deny-message field denies: never allow by accident while explaining a refusal.
      if (target.id === messageId) deny();
      else allow('once');
    } else {
      trapTab(e, box.current);
    }
  };

  return (
    <div
      ref={box}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby={titleId}
      aria-describedby={descId}
      className="flex max-h-[85vh] w-[min(620px,92vw)] flex-col rounded border border-[#f59e0b] bg-[#252526] shadow-xl"
      onMouseDown={(e) => e.stopPropagation()}
      onKeyDown={onKeyDown}
    >
      <div className="flex items-center gap-2 border-b border-[#3c3c3c] px-3 py-2 text-[13px]">
        <ShieldQuestion size={16} className="shrink-0 text-[#f59e0b]" />
        <span id={titleId} className="font-semibold">
          Allow {prompt.tool_name}?
        </span>
        <span className="ml-auto text-[11px] text-[#9a9a9a]" aria-label="queue position">
          {index + 1} of {total}
        </span>
      </div>
      <div className="min-h-0 flex-1 space-y-3 overflow-auto p-3 text-[13px]">
        <p id={descId} className="text-[#d4d4d4]">
          {prompt.run_id === ASSISTANT_RUN_ID ? 'The assistant' : 'A node'} wants to use a tool that needs your approval.
        </p>
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[12px]">
          <dt className="text-[#9a9a9a]">Node</dt>
          <dd className="min-w-0 truncate">
            {nodeTitle !== null && <span className="mr-2">{nodeTitle}</span>}
            <span className="font-mono text-[#9a9a9a]">{prompt.node_id}</span>
          </dd>
          <dt className="text-[#9a9a9a]">Tool</dt>
          <dd className="font-mono">{prompt.tool_name}</dd>
        </dl>
        <pre
          aria-label="request summary"
          className="max-h-40 overflow-auto whitespace-pre-wrap break-all rounded border border-[#3c3c3c] bg-[#1e1e1e] p-2 font-mono text-[12px]"
        >
          {prompt.summary}
        </pre>
        <details className="text-[12px]">
          <summary className="cursor-pointer text-[#9a9a9a]">Raw input</summary>
          <pre aria-label="raw input" className="mt-1 max-h-48 overflow-auto rounded bg-[#1e1e1e] p-2 font-mono text-[11px]">
            {prettyInput(prompt.input)}
          </pre>
        </details>
        <div>
          <label htmlFor={messageId} className="mb-1 block text-[11px] uppercase tracking-wide text-[#9a9a9a]">
            Message to Claude if you deny (optional)
          </label>
          <input
            id={messageId}
            className={inputCls}
            value={message}
            disabled={busy}
            onChange={(e) => setMessage(e.target.value)}
            placeholder="e.g. use the read-only tool instead"
          />
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          <button className={`${btnCls} mr-auto text-[#9a9a9a]`} onClick={onHide} disabled={busy} title="Hide this dialog; the top bar keeps the count">
            Decide later
          </button>
          <button className={`${btnCls} border border-[#ef4444] text-[#ef4444]`} onClick={deny} disabled={busy} title="Deny (Esc)">
            Deny
          </button>
          <button className={`${btnCls} border border-[#3c3c3c] ${unarmedCls}`} onClick={() => allow('graph')} disabled={busy} aria-disabled={!armed}>
            Always for this graph
          </button>
          <button className={`${btnCls} border border-[#3c3c3c] ${unarmedCls}`} onClick={() => allow('node')} disabled={busy} aria-disabled={!armed}>
            Allow for this node
          </button>
          <button
            className={`${btnCls} bg-[#b45309] text-white hover:bg-[#92400e] ${unarmedCls}`}
            onClick={() => allow('once')}
            disabled={busy}
            aria-disabled={!armed}
            title="Allow once (Enter)"
            data-autofocus
          >
            Allow once
          </button>
        </div>
      </div>
    </div>
  );
}

/** Backdrop; restores the focus it found when the last prompt is gone or the dialog is hidden. */
function PermissionModal({ children }: { children: React.ReactNode }) {
  // Captured during the first render: the child's autofocus effect runs before this component's effects.
  const [previous] = useState(() => document.activeElement as HTMLElement | null);
  useEffect(() => {
    return () => {
      if (previous && previous !== document.body && previous.isConnected && previous.matches?.(FOCUSABLE)) previous.focus();
    };
  }, [previous]);
  return <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">{children}</div>;
}

export function PermissionDialog() {
  const prompts = useGraphStore((s) => s.permissionPrompts);
  const hidden = useGraphStore((s) => s.permissionDialogHidden);
  const answering = useGraphStore((s) => s.permissionAnswering);
  const graph = useGraphStore((s) => s.graph);
  const answer = useGraphStore((s) => s.answerPermission);
  const hide = useGraphStore((s) => s.hidePermissionDialog);
  const prompt = prompts[0];
  if (prompt === undefined || hidden) return null;
  const nodeTitle =
    prompt.run_id === ASSISTANT_RUN_ID ? 'Assistant' : (graph?.nodes.find((n) => n.id === prompt.node_id)?.title ?? null);
  return (
    <PermissionModal>
      <PermissionDialogView
        key={prompt.id}
        prompt={prompt}
        index={0}
        total={prompts.length}
        nodeTitle={nodeTitle === '' ? null : nodeTitle}
        busy={answering !== null}
        onAnswer={(a) => void answer(a)}
        onHide={hide}
      />
    </PermissionModal>
  );
}

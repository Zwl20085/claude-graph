/** Message box of the Assistant tab: Ctrl+Enter or Send; disabled while the scheduler works. */
import { forwardRef, useState } from 'react';
import { Send } from 'lucide-react';
import type { AssistantStatus } from '../types/contract';
import { composerPlaceholder } from '../lib/assistantView';
import { btnCls, inputCls } from './ui';

interface AssistantComposerProps {
  status: AssistantStatus | null;
  busy: boolean;
  /** Resolves true when the message was accepted (the draft is then cleared). */
  onSend: (text: string) => Promise<boolean>;
}

export const AssistantComposer = forwardRef<HTMLTextAreaElement, AssistantComposerProps>(function AssistantComposer(
  { status, busy, onSend },
  ref,
) {
  const [draft, setDraft] = useState('');
  const working = status === 'working';
  const disabled = working || busy;
  const canSend = !disabled && draft.trim() !== '';

  const send = async (): Promise<void> => {
    if (!canSend) return;
    if (await onSend(draft)) setDraft('');
  };

  return (
    <div className="flex gap-1 border-t border-[#3c3c3c] p-2">
      <textarea
        ref={ref}
        aria-label="message to the scheduler"
        className={`${inputCls} resize-none`}
        rows={3}
        disabled={disabled}
        value={draft}
        placeholder={working ? 'The scheduler is working…' : composerPlaceholder(status)}
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
        disabled={!canSend}
        onClick={() => void send()}
        aria-label="send to the scheduler"
        title="Send (Ctrl+Enter)"
      >
        <Send size={13} />Send
      </button>
    </div>
  );
});

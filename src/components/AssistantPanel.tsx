/** Assistant tab: talk to the work scheduler session that runs the whole workflow. */
import { useEffect, useLayoutEffect, useRef } from 'react';
import { Play } from 'lucide-react';
import { api } from '../lib/api';
import { assistantStore, RUN_WORKFLOW_TEXT, useAssistantStore } from '../store/assistantStore';
import { graphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';
import { AssistantCard } from './AssistantCard';
import { AssistantComposer } from './AssistantComposer';
import { AssistantHeader } from './AssistantHeader';
import { btnCls } from './ui';

interface EmptyStateProps {
  disabled: boolean;
  onRun: () => void;
}

function EmptyState({ disabled, onRun }: EmptyStateProps) {
  return (
    <div className="space-y-2 p-1 text-[12px] text-[#9a9a9a]">
      <p className="text-[#d4d4d4]">
        The Assistant is one scheduler session that runs the whole graph for you and reports stages, questions and
        decisions here. To run node by node instead, use Run all in the top bar.
      </p>
      <button className={`${btnCls} border border-[#3b82f6] text-white`} disabled={disabled} onClick={onRun} aria-label="run the workflow">
        <Play size={13} />Run the workflow
      </button>
      <p className="text-[11px]">
        Model and permissions come from your Claude Code settings. Unattended runs need{' '}
        <code className="text-[#d4d4d4]">permissions.defaultMode: &quot;acceptEdits&quot;</code> or allow rules there.
      </p>
    </div>
  );
}

export function AssistantPanel() {
  const state = useAssistantStore((s) => s.state);
  const busy = useAssistantStore((s) => s.busy);
  const loadError = useAssistantStore((s) => s.error);
  const dir = useAssistantStore((s) => s.dir);
  const composer = useRef<HTMLTextAreaElement>(null);
  const timeline = useRef<HTMLDivElement>(null);
  const store = assistantStore().getState;
  const count = state?.messages.length ?? 0;
  const status = state?.session.status ?? null;

  useLayoutEffect(() => {
    const el = timeline.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [count]);

  // Waiting for an answer: put the cursor in the composer.
  useEffect(() => {
    if (status === 'waiting') composer.current?.focus();
  }, [status]);

  const openFolder = (): void => {
    if (!dir) return;
    api.revealGraphDir(dir).catch((err) => graphStore().getState().pushToast('error', `Could not open the folder: ${errorMessage(err)}`));
  };

  if (!state) {
    return <div className="p-3 text-[12px] text-[#777]">{loadError ? `Assistant unavailable: ${loadError}` : 'Loading the assistant…'}</div>;
  }
  return (
    <div className="flex h-full flex-col">
      <AssistantHeader
        session={state.session}
        busy={busy}
        onCancel={() => void store().cancel()}
        onReset={() => void store().reset()}
        onOpenFolder={openFolder}
      />
      <div ref={timeline} className="min-h-0 flex-1 space-y-2 overflow-auto p-2" aria-label="assistant timeline" role="log">
        {count === 0 ? (
          <EmptyState disabled={busy || status === 'working'} onRun={() => void store().send(RUN_WORKFLOW_TEXT)} />
        ) : (
          state.messages.map((m) => <AssistantCard key={m.id} message={m} onAnswer={() => composer.current?.focus()} />)
        )}
      </div>
      <AssistantComposer ref={composer} status={status} busy={busy} onSend={(text) => store().send(text)} />
    </div>
  );
}

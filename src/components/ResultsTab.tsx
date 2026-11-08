/** Past runs of the graph and this node's result in each. */
import { useEffect, useState } from 'react';
import { ExternalLink, SquareTerminal } from 'lucide-react';
import { deniedTools, questionsOf, sessionsOf, type NodeResult, type QuestionRecord, type RunSummary } from '../types/contract';
import { api } from '../lib/api';
import { DENIED_HINT, formatCost, STATUS_COLORS } from '../lib/status';
import { useGraphStore } from '../store/graphStore';
import { errorMessage, newestFirst, unionIds } from '../store/runHelpers';
import { isSessionAlive, sessionOfNode } from '../lib/sessions';
import { openSessionWindow } from './SessionChips';
import { btnCls } from './ui';

type Detail = { kind: 'none' } | { kind: 'loading' } | { kind: 'ok'; result: NodeResult } | { kind: 'error'; message: string };

function Counts({ run }: { run: RunSummary }) {
  return (
    <span className="flex gap-1.5">
      {Object.entries(run.counts)
        .filter(([, n]) => n > 0)
        .map(([s, n]) => (
          <span key={s} style={{ color: STATUS_COLORS[s as keyof typeof STATUS_COLORS] }} title={s}>
            {n} {s}
          </span>
        ))}
    </span>
  );
}

/** "Denied tools" line: shown whenever the list is non-empty. */
export function DeniedTools({ tools }: { tools: readonly string[] }) {
  if (tools.length === 0) return null;
  return (
    <p role="note" aria-label="denied tools" className="rounded border border-[#f59e0b] px-2 py-1 text-[12px] text-[#f59e0b]">
      Denied tools: {tools.join(', ')}. {DENIED_HINT}
    </p>
  );
}

/** How a recorded question was answered, in words. */
export function answerLine(q: QuestionRecord): string {
  if (q.offer) return 'offer: the node was done (deliverables written)';
  const a = q.answer;
  if (!a) return 'not answered';
  if (a.via === 'continue') return 'continued without answering';
  const what = a.text ?? (a.index !== undefined ? q.options[a.index]?.label ?? `option ${a.index + 1}` : null);
  const where = a.via === 'app' ? 'from the app' : 'in the window';
  return what ? `answered ${where}: ${what}` : `answered ${where}`;
}

/** The node's questions and dialogs of that run (questions.jsonl). */
export function QuestionList({ questions }: { questions: readonly QuestionRecord[] }) {
  if (questions.length === 0) return null;
  return (
    <div aria-label="questions" className="space-y-1">
      <p className="text-[11px] font-semibold text-[#38bdf8]">Questions ({questions.length})</p>
      <ul className="space-y-1">
        {questions.map((q) => (
          <li key={q.id} className="rounded border border-[#3c3c3c] px-2 py-1 text-[11px]">
            <p className="text-[#e5e5e5]">
              <span className="text-[#9a9a9a]">{q.kind === 'dialog' ? 'dialog' : 'question'}:</span> {q.question}
            </p>
            {q.options.length > 0 && <p className="text-[#9a9a9a]">{q.options.map((o, i) => `${i + 1}. ${o.label}`).join(' / ')}</p>}
            <p className="text-[#38bdf8]">{answerLine(q)}</p>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** "Open Claude Code window" while the node's session in the shown run is alive. */
function OpenSessionButton({ nodeId }: { nodeId: string }) {
  const runState = useGraphStore((s) => s.runState);
  const pushToast = useGraphStore((s) => s.pushToast);
  const session = sessionOfNode(sessionsOf(runState), nodeId);
  if (!runState || !session || !isSessionAlive(session)) return null;
  return (
    <button
      className={`${btnCls} border border-[#3c3c3c]`}
      onClick={() => openSessionWindow(runState.run_id, session.sid, (text) => pushToast('error', text))}
      title={`Open or focus the window of session ${session.title || session.sid}`}
    >
      <SquareTerminal size={13} />Open Claude Code window
    </button>
  );
}

export function ResultsTab({ nodeId }: { nodeId: string }) {
  const dir = useGraphStore((s) => s.dir);
  const runKey = useGraphStore((s) => `${s.runState?.run_id ?? ''}:${s.runState?.status ?? ''}`);
  const [runs, setRuns] = useState<RunSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [openRun, setOpenRun] = useState<string | null>(null);
  const [detail, setDetail] = useState<Detail>({ kind: 'none' });
  const pushToast = useGraphStore((s) => s.pushToast);
  const shownRunId = useGraphStore((s) => s.runState?.run_id ?? null);
  const shownDenials = useGraphStore((s) => s.runState?.nodes[nodeId]?.permission_denials);
  const reveal = (): void => {
    if (!dir) return;
    api.revealNodeDir(dir, nodeId).catch((err) => pushToast('error', `Could not open the folder: ${errorMessage(err)}`));
  };
  const openFolder = (
    <div className="flex flex-wrap gap-1">
      <OpenSessionButton nodeId={nodeId} />
      <button className={`${btnCls} border border-[#3c3c3c]`} onClick={reveal} title={`Show nodes/${nodeId}/deliverables in the file manager`}>
        <ExternalLink size={13} />Open deliverables folder
      </button>
    </div>
  );

  useEffect(() => {
    if (!dir) return;
    api
      .listRuns(dir)
      .then((r) => {
        setRuns(newestFirst(r));
        setError(null);
      })
      .catch((err) => setError(errorMessage(err)));
  }, [dir, runKey]);

  useEffect(() => {
    if (!dir || !openRun) return setDetail({ kind: 'none' });
    let live = true;
    setDetail({ kind: 'loading' });
    api
      .readNodeResult(dir, openRun, nodeId)
      .then((result) => live && setDetail({ kind: 'ok', result }))
      .catch((err) => live && setDetail({ kind: 'error', message: errorMessage(err) }));
    return () => {
      live = false;
    };
  }, [dir, openRun, nodeId]);

  if (error) return <p className="text-[12px] text-[#ef4444]">Could not list runs: {error}</p>;
  if (runs === null) return <p className="text-[12px] text-[#777]">Loading runs…</p>;
  if (runs.length === 0) return <div className="space-y-2">{openFolder}<p className="text-[12px] text-[#777]">No runs yet.</p></div>;

  return (
    <div className="space-y-2">
      {openFolder}
      <ul className="space-y-1">
        {runs.map((r) => (
          <li key={r.run_id}>
            <button
              className={`w-full rounded border px-2 py-1 text-left text-[11px] hover:bg-[#2d2d30] ${openRun === r.run_id ? 'border-[#3b82f6]' : 'border-[#3c3c3c]'}`}
              onClick={() => setOpenRun(openRun === r.run_id ? null : r.run_id)}
            >
              <div className="flex items-center gap-2">
                <span className="font-mono">{r.run_id}</span>
                <span className="ml-auto">{r.status}</span>
                <span>{formatCost(r.total_cost_usd)}</span>
              </div>
              <Counts run={r} />
            </button>
          </li>
        ))}
      </ul>
      {detail.kind === 'loading' && <p className="text-[12px] text-[#777]">Loading…</p>}
      {detail.kind === 'error' && <p className="text-[12px] text-[#ef4444]">{detail.message}</p>}
      {detail.kind === 'ok' && (
        <div className="space-y-1 text-[12px]">
          <DeniedTools
            tools={unionIds(
              deniedTools(detail.result.summary),
              openRun === shownRunId ? deniedTools({ permission_denials: shownDenials }) : [],
            )}
          />
          <QuestionList questions={questionsOf(detail.result)} />
          {detail.result.summary ? (
            <div className="grid grid-cols-2 gap-x-2 text-[11px] text-[#9a9a9a]">
              <span>status: {detail.result.summary.status}</span>
              <span>cost: {formatCost(detail.result.summary.cost_usd)}</span>
              <span>turns: {detail.result.summary.num_turns ?? '—'}</span>
              <span>duration: {detail.result.summary.duration_ms === null ? '—' : `${(detail.result.summary.duration_ms / 1000).toFixed(1)} s`}</span>
              <span className="col-span-2 truncate">session: {detail.result.summary.session_id ?? '—'}</span>
            </div>
          ) : (
            <p className="text-[#777]">No summary.json for this node in that run.</p>
          )}
          <pre className="max-h-[50vh] overflow-auto whitespace-pre-wrap rounded bg-[#1e1e1e] p-2 font-mono text-[11px]">
            {detail.result.result_md ?? '(no result.md)'}
          </pre>
        </div>
      )}
    </div>
  );
}

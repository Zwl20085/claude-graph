/** This node across the graph's runs: its status, duration, cost and questions per run (latest opened), its result rendered as markdown. */
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
import { Markdown } from '../lib/markdown';


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
      <p className="text-[11px] font-semibold text-[#a78bfa]">Questions ({questions.length})</p>
      <ul className="space-y-1">
        {questions.map((q) => (
          <li key={q.id} className="rounded border border-[#3c3c3c] px-2 py-1 text-[11px]">
            <p className="text-[#e5e5e5]">
              <span className="text-[#9a9a9a]">{q.kind === 'dialog' ? 'dialog' : 'question'}:</span> {q.question}
            </p>
            {q.options.length > 0 && <p className="text-[#9a9a9a]">{q.options.map((o, i) => `${i + 1}. ${o.label}`).join(' / ')}</p>}
            <p className="text-[#a78bfa]">{answerLine(q)}</p>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** "Open session window" while the node's session in the shown run is alive. */
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
      <SquareTerminal size={13} />Open session window
    </button>
  );
}

/** Runs looked up for one node (newest first). */
export const RESULT_RUNS_MAX = 15;

/** One run in which this node took part. */
export interface NodeRun {
  run: RunSummary;
  result: NodeResult;
}

function seconds(ms: number | null | undefined): string {
  return ms === null || ms === undefined ? '—' : `${(ms / 1000).toFixed(1)} s`;
}

/** The node's own row: its status, duration, cost and questions in that run. */
function NodeRunRow({ item, open, onToggle }: { item: NodeRun; open: boolean; onToggle: () => void }) {
  const summary = item.result.summary;
  const status = summary?.status ?? null;
  const questions = questionsOf(item.result).length;
  return (
    <button
      className={`w-full rounded border px-2 py-1 text-left text-[11px] hover:bg-[#2d2d30] ${open ? 'border-[#3b82f6]' : 'border-[#3c3c3c]'}`}
      aria-expanded={open}
      onClick={onToggle}
    >
      <div className="flex items-center gap-2">
        <span className="font-mono text-[#9a9a9a]">{item.run.run_id}</span>
        {status && (
          <span className="rounded px-1 font-semibold text-black" style={{ background: STATUS_COLORS[status as keyof typeof STATUS_COLORS] ?? '#777' }}>
            {status}
          </span>
        )}
        {questions > 0 && (
          <span className="rounded px-1 text-black" style={{ background: STATUS_COLORS.waiting }} aria-label={`${questions} questions`}>
            {questions}?
          </span>
        )}
        <span className="ml-auto">{seconds(summary?.duration_ms)}</span>
        <span>{formatCost(summary?.cost_usd ?? null)}</span>
      </div>
    </button>
  );
}

export function ResultsTab({ nodeId }: { nodeId: string }) {
  const dir = useGraphStore((s) => s.dir);
  const runKey = useGraphStore((s) => `${s.runState?.run_id ?? ''}:${s.runState?.status ?? ''}`);
  const [items, setItems] = useState<NodeRun[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [openRun, setOpenRun] = useState<string | null>(null);
  const pushToast = useGraphStore((s) => s.pushToast);
  const shownRunId = useGraphStore((s) => s.runState?.run_id ?? null);
  const shownDenials = useGraphStore((s) => s.runState?.nodes[nodeId]?.permission_denials);
  const reveal = (): void => {
    if (!dir) return;
    api.revealNodeDir(dir, nodeId).catch((err) => pushToast('error', `Could not open the folder: ${errorMessage(err)}`));
  };

  useEffect(() => {
    if (!dir) return;
    let live = true;
    (async () => {
      const runs = newestFirst(await api.listRuns(dir)).slice(0, RESULT_RUNS_MAX);
      const results = await Promise.all(
        runs.map(async (run) => {
          try {
            return { run, result: await api.readNodeResult(dir, run.run_id, nodeId) };
          } catch {
            return null;
          }
        }),
      );
      const took = results.filter((x): x is NodeRun => x !== null && (x.result.summary !== null || x.result.result_md !== null));
      if (!live) return;
      setItems(took);
      setError(null);
      // The latest run opens by itself.
      setOpenRun((cur) => (cur !== null && took.some((t) => t.run.run_id === cur) ? cur : (took[0]?.run.run_id ?? null)));
    })().catch((err) => live && setError(errorMessage(err)));
    return () => {
      live = false;
    };
  }, [dir, runKey, nodeId]);

  const live = <OpenSessionButton nodeId={nodeId} />;
  if (error) return <p className="text-[12px] text-[#ef4444]">Could not list runs: {error}</p>;
  if (items === null) return <div className="space-y-2">{live}<p className="text-[12px] text-[#777]">Loading runs…</p></div>;
  if (items.length === 0) return <div className="space-y-2">{live}<p className="text-[12px] text-[#777]">This node has not run yet.</p></div>;
  const shown = items.find((i) => i.run.run_id === openRun) ?? null;

  return (
    <div className="space-y-2">
      {live}
      <ul className="space-y-1" aria-label="runs of this node">
        {items.map((it) => (
          <li key={it.run.run_id}>
            <NodeRunRow item={it} open={openRun === it.run.run_id} onToggle={() => setOpenRun(openRun === it.run.run_id ? null : it.run.run_id)} />
          </li>
        ))}
      </ul>
      {shown && (
        <div className="space-y-1.5 text-[12px]" aria-label="run detail">
          <div className="flex flex-wrap gap-1">
            <button className={`${btnCls} border border-[#3c3c3c]`} onClick={reveal} title={`Show nodes/${nodeId}/deliverables in the file manager`}>
              <ExternalLink size={13} />Open deliverables folder
            </button>
          </div>
          <DeniedTools
            tools={unionIds(
              deniedTools(shown.result.summary),
              shown.run.run_id === shownRunId ? deniedTools({ permission_denials: shownDenials }) : [],
            )}
          />
          {shown.result.summary && (
            <div className="grid grid-cols-2 gap-x-2 text-[11px] text-[#9a9a9a]">
              <span>status: {shown.result.summary.status}</span>
              <span>cost: {formatCost(shown.result.summary.cost_usd)}</span>
              <span>turns: {shown.result.summary.num_turns ?? '—'}</span>
              <span>duration: {seconds(shown.result.summary.duration_ms)}</span>
              <span className="col-span-2 truncate">session: {shown.result.summary.session_id ?? '—'}</span>
            </div>
          )}
          <QuestionList questions={questionsOf(shown.result)} />
          {shown.result.result_md ? <Markdown text={shown.result.result_md} /> : <p className="text-[#777]">(no result.md)</p>}
        </div>
      )}
    </div>
  );
}

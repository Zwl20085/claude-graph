/**
 * Messages tab of an agent-driven run: the conversation lives in the user's
 * CLI, so the node's questions (questions.jsonl, from `blocked` reports) are
 * shown read-only with "Answer in your CLI".
 */
import { useEffect, useState } from 'react';
import type { QuestionRecord } from '../types/contract';
import { api } from '../lib/api';
import { useGraphStore } from '../store/graphStore';

export const ANSWER_IN_CLI = 'Answer in your CLI';

export function AgentMessages({ nodeId, client }: { nodeId: string; client: string }) {
  const dir = useGraphStore((s) => s.dir);
  const runId = useGraphStore((s) => s.runState?.run_id ?? null);
  const status = useGraphStore((s) => s.runState?.nodes[nodeId]?.status ?? null);
  const [questions, setQuestions] = useState<readonly QuestionRecord[]>([]);

  useEffect(() => {
    if (!dir || !runId) return setQuestions([]);
    let live = true;
    api
      .readNodeResult(dir, runId, nodeId)
      .then((r) => live && setQuestions(r.questions ?? []))
      .catch((err) => {
        console.warn('read_node_result failed', err);
        if (live) setQuestions([]);
      });
    return () => {
      live = false;
    };
  }, [dir, runId, nodeId, status]);

  return (
    <div className="flex h-full flex-col gap-2">
      {questions.length === 0 && (
        <p className="text-[12px] text-[#777]">This run is driven by {client} in your CLI; the conversation is there.</p>
      )}
      {questions.map((q) => (
        <div key={q.id} className="rounded border border-[#38bdf8]/40 bg-[#0c4a6e]/30 p-2 text-[12px]" aria-label="agent question">
          <p className="whitespace-pre-wrap break-words text-[#e5e5e5]">{q.question}</p>
          <p className="mt-1 text-[11px] text-[#38bdf8]">
            {q.answer ? 'Answered.' : `${ANSWER_IN_CLI} (${client}), then ask it to retry this node.`}
          </p>
        </div>
      ))}
    </div>
  );
}

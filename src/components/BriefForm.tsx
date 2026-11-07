/** Structured brief (recommended): every template field of a node. */
import { useEffect, useRef, useState } from 'react';
import { FolderSearch } from 'lucide-react';
import type { Node } from '../types/contract';
import { api } from '../lib/api';
import { readToolList, upstreamOf } from '../lib/graphOps';
import { continueSessionError, workingDirError } from '../lib/validate';
import { useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';
import { DeliverablesEditor, StringListEditor } from './ListEditor';
import { btnCls, Field, Input, inputCls, NumberInput, TextArea, useFieldA11y } from './ui';

const joinTools = (tools: readonly string[] | null): string => (tools ?? []).join(', ');

function ToolsInput({ node, onError }: { node: Node; onError: (e: string | null) => void }) {
  const update = useGraphStore((s) => s.updateNode);
  const [text, setText] = useState(joinTools(node.allowed_tools));
  const committed = useRef(joinTools(node.allowed_tools));
  const a11y = useFieldA11y();

  // Re-sync when the node (or its tools) changes from outside this input.
  useEffect(() => {
    const now = joinTools(node.allowed_tools);
    if (now === committed.current) return;
    committed.current = now;
    setText(now);
    onError(null);
  }, [node.id, node.allowed_tools, onError]);

  return (
    <input
      className={`${inputCls} font-mono`}
      value={text}
      placeholder="Read, Write, Bash(python:*)"
      {...a11y}
      onChange={(e) => {
        setText(e.target.value);
        const { valid, error } = readToolList(e.target.value);
        onError(error);
        committed.current = joinTools(valid);
        update(node.id, { allowed_tools: valid });
      }}
    />
  );
}

function WorkingDirInput({ node }: { node: Node }) {
  const update = useGraphStore((s) => s.updateNode);
  const pushToast = useGraphStore((s) => s.pushToast);
  const [draft, setDraft] = useState(node.working_dir ?? '');
  useEffect(() => {
    setDraft((d) => ((d.trim() === '' ? null : d) === node.working_dir ? d : (node.working_dir ?? '')));
  }, [node.working_dir]);
  const value = draft.trim() === '' ? null : draft;
  const error = workingDirError(value);

  const browse = async (): Promise<void> => {
    try {
      const dir = await api.pickFolder('Working directory for this node');
      if (dir) {
        setDraft(dir);
        if (workingDirError(dir) === null) update(node.id, { working_dir: dir });
      }
    } catch (err) {
      pushToast('error', `Folder picker failed: ${errorMessage(err)}`);
    }
  };

  return (
    <Field label="Working directory" hint="Empty = graph folder. Relative to the graph folder, or absolute (needs trust)." error={error ? `${error} Not saved until fixed.` : null}>
      <div className="flex gap-1">
        <Input
          className={`${inputCls} font-mono ${error ? 'border-[#ef4444]' : ''}`}
          value={draft}
          aria-invalid={error !== null}
          onChange={(e) => {
            setDraft(e.target.value);
            const next = e.target.value.trim() === '' ? null : e.target.value;
            if (workingDirError(next) === null) update(node.id, { working_dir: next });
          }}
        />
        <button className={`${btnCls} border border-[#3c3c3c]`} onClick={() => void browse()} title="Browse" aria-label="browse for working directory">
          <FolderSearch size={13} />
        </button>
      </div>
    </Field>
  );
}

export function BriefForm({ node }: { node: Node }) {
  const update = useGraphStore((s) => s.updateNode);
  const graph = useGraphStore((s) => s.graph);
  const [toolsError, setToolsError] = useState<string | null>(null);
  const set = (patch: Parameters<typeof update>[1]): void => update(node.id, patch);
  const upstream = graph ? upstreamOf(graph, node.id) : [];
  const sessionError = continueSessionError(node.continue_session_of, upstream);

  return (
    <div className="space-y-3">
      <p className="rounded border border-[#3c3c3c] bg-[#1e1e1e] px-2 py-1.5 text-[12px] text-[#9a9a9a]">
        Optional. Filling this makes the node checkable: the app verifies deliverables exist and feeds them to downstream nodes.
      </p>
      <Field label="Goal"><TextArea rows={2} value={node.goal} onChange={(e) => set({ goal: e.target.value })} /></Field>
      <Field label="Acceptance criterion"><TextArea rows={2} value={node.acceptance} onChange={(e) => set({ acceptance: e.target.value })} /></Field>
      <Field label="Scope in"><TextArea rows={2} value={node.scope_in} onChange={(e) => set({ scope_in: e.target.value })} /></Field>
      <Field label="Scope out"><TextArea rows={2} value={node.scope_out} onChange={(e) => set({ scope_out: e.target.value })} /></Field>
      <Field label="Already known / ruled out">
        <TextArea rows={2} value={node.already_known} onChange={(e) => set({ already_known: e.target.value })} />
      </Field>
      <Field label="Deliverables" hint="Paths relative to nodes/<id>/deliverables/" composite>
        <DeliverablesEditor items={node.deliverables} onChange={(deliverables) => set({ deliverables })} />
      </Field>
      <Field label="Do not" composite>
        <StringListEditor items={node.do_nots} onChange={(do_nots) => set({ do_nots })} placeholder="Do not …" itemLabel="do-not" />
      </Field>
      <Field label="Allowed tools" hint="Comma separated; empty = CLI default" error={toolsError}>
        <ToolsInput node={node} onError={setToolsError} />
      </Field>
      <div className="grid grid-cols-2 gap-2">
        <Field label="Max turns">
          <NumberInput value={node.max_turns} min={1} nullable placeholder="none" onCommit={(max_turns) => set({ max_turns })} />
        </Field>
        <Field label="Continue session of" error={sessionError}>
          <SessionSelect value={node.continue_session_of} upstream={upstream} onChange={(continue_session_of) => set({ continue_session_of })} />
        </Field>
      </div>
      <WorkingDirInput node={node} />
      <Field label="Extra context"><TextArea rows={3} value={node.extra_context} onChange={(e) => set({ extra_context: e.target.value })} /></Field>
    </div>
  );
}

interface SessionSelectProps {
  value: string | null;
  upstream: readonly string[];
  onChange: (value: string | null) => void;
}

/** Only direct upstream nodes are offered; a stale value stays visible (flagged by the Field). */
function SessionSelect({ value, upstream, onChange }: SessionSelectProps) {
  const a11y = useFieldA11y();
  return (
    <select className={inputCls} value={value ?? ''} {...a11y} onChange={(e) => onChange(e.target.value === '' ? null : e.target.value)}>
      <option value="">(new session)</option>
      {value !== null && !upstream.includes(value) && <option value={value}>{value} (not upstream)</option>}
      {upstream.map((id) => (
        <option key={id} value={id}>{id}</option>
      ))}
    </select>
  );
}

/** Structured brief (recommended): every template field of a node. */
import { useState } from 'react';
import { FolderSearch } from 'lucide-react';
import type { Node } from '../types/contract';
import { api } from '../lib/api';
import { parseToolList, upstreamOf } from '../lib/graphOps';
import { useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';
import { DeliverablesEditor, StringListEditor } from './ListEditor';
import { btnCls, Field, inputCls } from './ui';

function ToolsInput({ node }: { node: Node }) {
  const update = useGraphStore((s) => s.updateNode);
  const [text, setText] = useState((node.allowed_tools ?? []).join(', '));
  return (
    <input
      className={`${inputCls} font-mono`}
      value={text}
      placeholder="Read, Write, Bash(python:*)"
      onChange={(e) => {
        setText(e.target.value);
        update(node.id, { allowed_tools: parseToolList(e.target.value) });
      }}
    />
  );
}

function TextArea({ value, onChange, rows = 2 }: { value: string; onChange: (v: string) => void; rows?: number }) {
  return <textarea className={`${inputCls} resize-y`} rows={rows} value={value} onChange={(e) => onChange(e.target.value)} />;
}

export function BriefForm({ node }: { node: Node }) {
  const update = useGraphStore((s) => s.updateNode);
  const graph = useGraphStore((s) => s.graph);
  const pushToast = useGraphStore((s) => s.pushToast);
  const set = (patch: Parameters<typeof update>[1]): void => update(node.id, patch);
  const upstream = graph ? upstreamOf(graph, node.id) : [];

  const browse = async (): Promise<void> => {
    try {
      const dir = await api.pickFolder('Working directory for this node');
      if (dir) set({ working_dir: dir });
    } catch (err) {
      pushToast('error', `Folder picker failed: ${errorMessage(err)}`);
    }
  };

  return (
    <div className="space-y-3">
      <p className="rounded border border-[#3c3c3c] bg-[#1e1e1e] px-2 py-1.5 text-[12px] text-[#9a9a9a]">
        Optional. Filling this makes the node checkable: the app verifies deliverables exist and feeds them to downstream nodes.
      </p>
      <Field label="Goal"><TextArea value={node.goal} onChange={(goal) => set({ goal })} /></Field>
      <Field label="Acceptance criterion"><TextArea value={node.acceptance} onChange={(acceptance) => set({ acceptance })} /></Field>
      <Field label="Scope in"><TextArea value={node.scope_in} onChange={(scope_in) => set({ scope_in })} /></Field>
      <Field label="Scope out"><TextArea value={node.scope_out} onChange={(scope_out) => set({ scope_out })} /></Field>
      <Field label="Already known / ruled out"><TextArea value={node.already_known} onChange={(already_known) => set({ already_known })} /></Field>
      <Field label="Deliverables" hint="Paths relative to nodes/<id>/deliverables/">
        <DeliverablesEditor items={node.deliverables} onChange={(deliverables) => set({ deliverables })} />
      </Field>
      <Field label="Do not">
        <StringListEditor items={node.do_nots} onChange={(do_nots) => set({ do_nots })} placeholder="Do not …" />
      </Field>
      <Field label="Allowed tools" hint="Comma separated; empty = CLI default">
        <ToolsInput key={node.id} node={node} />
      </Field>
      <div className="grid grid-cols-2 gap-2">
        <Field label="Max turns">
          <input
            type="number"
            min={1}
            className={inputCls}
            value={node.max_turns ?? ''}
            placeholder="none"
            onChange={(e) => {
              const n = Number.parseInt(e.target.value, 10);
              set({ max_turns: Number.isFinite(n) && n > 0 ? n : null });
            }}
          />
        </Field>
        <Field label="Continue session of">
          <select
            className={inputCls}
            value={node.continue_session_of ?? ''}
            onChange={(e) => set({ continue_session_of: e.target.value === '' ? null : e.target.value })}
          >
            <option value="">(new session)</option>
            {upstream.map((id) => (
              <option key={id} value={id}>{id}</option>
            ))}
          </select>
        </Field>
      </div>
      <Field label="Working directory" hint="Empty = graph folder">
        <div className="flex gap-1">
          <input
            className={`${inputCls} font-mono`}
            value={node.working_dir ?? ''}
            onChange={(e) => set({ working_dir: e.target.value.trim() === '' ? null : e.target.value })}
          />
          <button className={`${btnCls} border border-[#3c3c3c]`} onClick={() => void browse()} title="Browse">
            <FolderSearch size={13} />
          </button>
        </div>
      </Field>
      <Field label="Extra context"><TextArea rows={3} value={node.extra_context} onChange={(extra_context) => set({ extra_context })} /></Field>
    </div>
  );
}

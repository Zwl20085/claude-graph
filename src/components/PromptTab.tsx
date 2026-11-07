/** Simple mode: one prompt box plus model / permission / resources. */
import { MODELS, PERMISSION_MODES, type Node } from '../types/contract';
import { useGraphStore } from '../store/graphStore';
import { ChipsInput, Field, inputCls, Select } from './ui';

const MIN_ROWS = 12;

export function PromptTab({ node }: { node: Node }) {
  const update = useGraphStore((s) => s.updateNode);
  const rows = Math.max(MIN_ROWS, node.prompt.split('\n').length + 1);
  return (
    <div className="space-y-3">
      <textarea
        className={`${inputCls} resize-y font-mono leading-5`}
        rows={rows}
        value={node.prompt}
        placeholder="What would you type to Claude Code for this step?"
        aria-label="Prompt"
        onChange={(e) => update(node.id, { prompt: e.target.value })}
      />
      <div className="grid grid-cols-2 gap-2">
        <Field label="Model">
          <Select value={node.model} options={MODELS} onChange={(model) => update(node.id, { model })} />
        </Field>
        <Field label="Permission">
          <Select value={node.permission_mode} options={PERMISSION_MODES} onChange={(permission_mode) => update(node.id, { permission_mode })} />
        </Field>
      </div>
      {node.permission_mode === 'bypassPermissions' && (
        <p className="text-[11px] text-[#ef4444]">bypassPermissions lets this node run any tool without asking.</p>
      )}
      <Field label="Resources">
        <ChipsInput values={node.resources} onChange={(resources) => update(node.id, { resources })} />
      </Field>
    </div>
  );
}

/**
 * Simple mode: one prompt box plus harness / model / permission / resources.
 * In terminal mode (interactive session windows) the headless-only fields
 * are disabled and tagged "headless only".
 */
import { effectiveHarness, harnessOf, MODELS, PERMISSION_MODES, runModeOf, type Node } from '../types/contract';
import { harnessLabel, isClaudeKind, NOT_CLAUDE_MODEL_NOTE } from '../lib/harness';
import { ASK_APP_HINT, permissionLabel } from '../lib/status';
import { useGraphStore } from '../store/graphStore';
import { HarnessSelect } from './HarnessSelect';
import { ChipsInput, Field, inputCls, Select } from './ui';

const MIN_ROWS = 12;
export const HEADLESS_ONLY_NOTE = 'Permission mode, tools, turns and resource tags apply to the headless engine only.';
export const HEADLESS_ONLY = 'headless only';

/** The small "headless only" tag next to a field that terminal mode ignores. */
export function HeadlessTag() {
  return (
    <span className="ml-1 rounded bg-[#37373d] px-1 text-[10px] normal-case tracking-normal text-[#9a9a9a]" aria-label={HEADLESS_ONLY}>
      {HEADLESS_ONLY}
    </span>
  );
}

export function PromptTab({ node }: { node: Node }) {
  const update = useGraphStore((s) => s.updateNode);
  const terminal = useGraphStore((s) => runModeOf(s.graph?.settings) === 'terminal');
  const inheritLabel = useGraphStore((s) => harnessLabel(s.harnesses, harnessOf(s.graph?.settings)));
  const claudeModel = useGraphStore((s) => isClaudeKind(s.harnesses, effectiveHarness(node, s.graph?.settings)));
  const rows = Math.max(MIN_ROWS, node.prompt.split('\n').length + 1);
  return (
    <div className="space-y-3">
      <textarea
        className={`${inputCls} resize-y font-mono leading-5`}
        rows={rows}
        value={node.prompt}
        placeholder="What should this step do?"
        aria-label="Prompt"
        onChange={(e) => update(node.id, { prompt: e.target.value })}
      />
      <Field label="Harness">
        <HarnessSelect value={node.harness} inheritLabel={inheritLabel} onChange={(harness) => update(node.id, { harness })} />
      </Field>
      <div className="grid grid-cols-2 gap-2">
        <Field label="Model" hint={claudeModel ? undefined : NOT_CLAUDE_MODEL_NOTE}>
          <Select value={node.model} options={MODELS} disabled={!claudeModel} onChange={(model) => update(node.id, { model })} />
        </Field>
        <Field label="Permission" hint={terminal ? HEADLESS_ONLY : undefined}>
          <Select
            value={node.permission_mode}
            options={PERMISSION_MODES}
            label={permissionLabel}
            disabled={terminal}
            onChange={(permission_mode) => update(node.id, { permission_mode })}
          />
        </Field>
      </div>
      {!terminal && node.permission_mode === 'ask_app' && <p className="text-[11px] text-[#f59e0b]">{ASK_APP_HINT}</p>}
      {!terminal && node.permission_mode === 'bypassPermissions' && (
        <p className="text-[11px] text-[#ef4444]">bypassPermissions lets this node run any tool without asking.</p>
      )}
      <Field label="Resources" hint={terminal ? HEADLESS_ONLY : undefined}>
        {terminal ? (
          <p className="text-[12px] text-[#777]" aria-label="resources (headless only)">
            {node.resources.length > 0 ? node.resources.join(', ') : 'none'}
          </p>
        ) : (
          <ChipsInput values={node.resources} onChange={(resources) => update(node.id, { resources })} />
        )}
      </Field>
      {terminal && (
        <p role="note" aria-label="terminal mode note" className="text-[11px] text-[#8a8a8a]">
          {HEADLESS_ONLY_NOTE}
        </p>
      )}
    </div>
  );
}

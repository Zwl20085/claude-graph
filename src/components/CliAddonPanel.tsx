/**
 * Settings → "Use from your CLI": how to register `claude-graph --mcp-graph`
 * with Claude Code, Codex and dsh. Copy buttons for the snippets; "Add to
 * <client>" first shows exactly what it will run or write and asks.
 */
import { useEffect, useState } from 'react';
import { Copy, Plus } from 'lucide-react';
import type { AgentClientConfig } from '../types/agent';
import { agentApi, type AgentApi } from '../lib/agentApi';
import { errorMessage } from '../store/runHelpers';
import { btnCls, errorCls } from './ui';

export const CLI_ADDON_TITLE = 'Use from your CLI';

function ClientRow({ config, api, onDone }: { config: AgentClientConfig; api: AgentApi; onDone: () => void }) {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const copy = (): void => {
    void navigator.clipboard?.writeText(config.snippet).catch((err) => setResult({ ok: false, text: errorMessage(err) }));
  };
  const add = async (): Promise<void> => {
    setAsking(false);
    setBusy(true);
    try {
      setResult({ ok: true, text: (await api.agentInstall(config.id)) || 'Added.' });
      onDone();
    } catch (err) {
      setResult({ ok: false, text: errorMessage(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-1" aria-label={`${config.name} setup`}>
      <div className="flex items-center gap-1">
        <span className="flex-1 text-[12px] font-semibold">{config.name}</span>
        {config.installed && <span className="text-[11px] text-[#22c55e]">added</span>}
        <button className={btnCls} onClick={copy} aria-label={`copy the ${config.name} snippet`} title="Copy">
          <Copy size={12} />
        </button>
        {!config.installed && (
          <button className={btnCls} disabled={busy} onClick={() => setAsking(true)} aria-label={`add to ${config.name}`}>
            <Plus size={12} />
            Add
          </button>
        )}
      </div>
      <pre className="max-h-28 overflow-auto whitespace-pre-wrap break-all rounded bg-[#1e1e1e] p-1.5 font-mono text-[10px] text-[#d4d4d4]">
        {config.snippet}
      </pre>
      {asking && (
        <div className="rounded border border-[#f59e0b]/50 p-1.5 text-[11px]" role="alertdialog" aria-label={`confirm add to ${config.name}`}>
          <p className="break-all">This will {config.add_action}.</p>
          <div className="mt-1 flex gap-1">
            <button className={`${btnCls} bg-[#3b82f6] text-white`} onClick={() => void add()}>
              Add
            </button>
            <button className={btnCls} onClick={() => setAsking(false)}>
              Cancel
            </button>
          </div>
        </div>
      )}
      {result && <span className={result.ok ? 'block text-[11px] text-[#22c55e]' : errorCls}>{result.text}</span>}
    </div>
  );
}

export function CliAddonPanel({ api = agentApi }: { api?: AgentApi }) {
  const [configs, setConfigs] = useState<readonly AgentClientConfig[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  const load = (): void => {
    api
      .agentClientConfigs()
      .then((c) => {
        setConfigs(c);
        setError(null);
      })
      .catch((err) => setError(errorMessage(err)));
  };
  useEffect(() => {
    if (open && configs === null) load();
  }, [open]);

  return (
    <section className="space-y-2 border-t border-[#3c3c3c] pt-3" aria-label={CLI_ADDON_TITLE}>
      <button className="flex w-full items-center justify-between text-left" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-[#d4d4d4]">{CLI_ADDON_TITLE}</h3>
        <span className="text-[11px] text-[#9a9a9a]">{open ? 'hide' : 'show'}</span>
      </button>
      {open && (
        <>
          <p className="text-[11px] text-[#9a9a9a]">
            Let Claude Code, Codex or dsh run this graph from their own session: they call the claude-graph tools
            (graph_open, graph_next, graph_report) and this window follows live.
          </p>
          {configs === null && !error && <p className="text-[11px] text-[#9a9a9a]">checking…</p>}
          {error && <span className={errorCls}>{error}</span>}
          {configs?.map((c) => <ClientRow key={c.id} config={c} api={api} onDone={load} />)}
        </>
      )}
    </section>
  );
}

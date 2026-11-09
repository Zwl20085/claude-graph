/** Shown when no graph is open: recent graphs, open a folder, or create a new graph. */
import { useState } from 'react';
import { FilePlus, FolderOpen, History } from 'lucide-react';
import { graphStore, useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';
import { api } from '../lib/api';
import { btnCls, inputCls } from './ui';
import { InboxButton } from './InboxPanel';

async function pick(title: string): Promise<string | null> {
  try {
    return await api.pickFolder(title);
  } catch (err) {
    graphStore().getState().pushToast('error', `Folder picker failed: ${errorMessage(err)}`);
    return null;
  }
}

export async function openFolderFlow(): Promise<void> {
  const dir = await pick('Open a graph folder');
  if (dir) await graphStore().getState().openGraph(dir);
}

const baseName = (dir: string): string => dir.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? 'graph';

export async function newGraphFlow(name?: string): Promise<void> {
  const dir = await pick('Choose a folder for the new graph');
  if (!dir) return;
  const graphName = name?.trim() || baseName(dir);
  await graphStore().getState().createGraph(dir, graphName);
}

export function StartScreen() {
  const recent = useGraphStore((s) => s.recent);
  const [name, setName] = useState('');
  return (
    <div className="flex h-full items-center justify-center bg-[#1e1e1e]">
      <div className="w-[520px] space-y-5 rounded border border-[#3c3c3c] bg-[#252526] p-6">
        <div>
          <h1 className="text-lg font-semibold text-[#d4d4d4]">Claude Graph</h1>
          <p className="text-[13px] text-[#9a9a9a]">Draw a graph of agent tasks and run them in interactive session windows (Claude Code by default; Codex, dsh or your own CLI per node).</p>
        </div>
        <div className="flex gap-2">
          <input className={inputCls} placeholder="New graph name" value={name} onChange={(e) => setName(e.target.value)} />
          <button className={`${btnCls} shrink-0 bg-[#3b82f6] text-white hover:bg-[#2563eb]`} onClick={() => void newGraphFlow(name)}>
            <FilePlus size={14} />New graph
          </button>
          <button className={`${btnCls} shrink-0 border border-[#3c3c3c]`} onClick={() => void openFolderFlow()}>
            <FolderOpen size={14} />Open folder
          </button>
          <span className="shrink-0 rounded border border-[#3c3c3c]">
            <InboxButton />
          </span>
        </div>
        <div>
          <h2 className="mb-1 flex items-center gap-1 text-[11px] uppercase tracking-wide text-[#9a9a9a]">
            <History size={12} />Recent graphs
          </h2>
          {recent.length === 0 ? (
            <p className="text-[12px] text-[#777]">No recent graphs.</p>
          ) : (
            <ul className="space-y-0.5">
              {recent.map((dir) => (
                <li key={dir}>
                  <button
                    className="w-full truncate rounded px-2 py-1 text-left font-mono text-[12px] hover:bg-[#37373d]"
                    title={dir}
                    onClick={() => void graphStore().getState().openGraph(dir)}
                  >
                    {dir}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}

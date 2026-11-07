/** Top bar: graph name, file actions, run controls, settings, claude status. */
import { useState } from 'react';
import { useReactFlow } from '@xyflow/react';
import {
  FilePlus,
  FolderOpen,
  PanelBottom,
  Play,
  PlayCircle,
  Plus,
  RotateCcw,
  Save,
  Settings as SettingsIcon,
  Square,
} from 'lucide-react';
import { MOCK_MODE } from '../lib/api';
import { isRunActive, runHasFailures } from '../lib/status';
import { graphStore, useGraphStore, type ClaudeState } from '../store/graphStore';
import { GRID } from './Canvas';
import { SettingsPopover } from './SettingsPopover';
import { btnCls } from './ui';
import { newGraphFlow, openFolderFlow } from './StartScreen';

function ClaudeChip({ state }: { state: ClaudeState }) {
  if (state.kind === 'checking') return <span className="text-[11px] text-[#8a8a8a]">checking claude…</span>;
  const ok = state.kind === 'ok' && state.check.found;
  const text = ok
    ? `claude ${state.kind === 'ok' ? (state.check.version ?? '') : ''}`.trim()
    : 'claude not found — install Claude Code and log in';
  const title = state.kind === 'ok' ? (state.check.path ?? undefined) : state.message;
  return (
    <span
      title={title}
      className={`rounded-full px-2 py-0.5 text-[11px] ${ok ? 'bg-[#22c55e]/15 text-[#22c55e]' : 'bg-[#ef4444]/15 text-[#ef4444]'}`}
    >
      {text}
    </span>
  );
}

interface TopBarProps {
  logVisible: boolean;
  onToggleLog: () => void;
}

export function TopBar({ logVisible, onToggleLog }: TopBarProps) {
  const name = useGraphStore((s) => s.graph?.name ?? '');
  const dirty = useGraphStore((s) => s.dirty);
  const runState = useGraphStore((s) => s.runState);
  const selected = useGraphStore((s) => s.selectedNodeId);
  const claude = useGraphStore((s) => s.claude);
  const starting = useGraphStore((s) => s.startingRun);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const { screenToFlowPosition } = useReactFlow();
  const store = graphStore().getState;
  const active = isRunActive(runState) || starting;

  const addNode = (): void => {
    const p = screenToFlowPosition({ x: window.innerWidth * 0.4, y: window.innerHeight * 0.4 });
    store().addNodeAt({ x: Math.round(p.x / GRID) * GRID, y: Math.round(p.y / GRID) * GRID });
  };

  return (
    <header className="flex h-10 shrink-0 items-center gap-1 border-b border-[#3c3c3c] bg-[#252526] px-2 text-[13px]">
      <span className="mr-1 font-semibold text-[#3b82f6]">Claude Graph</span>
      <input
        className="w-48 rounded bg-transparent px-1 py-0.5 outline-none hover:bg-[#1e1e1e] focus:bg-[#1e1e1e]"
        value={name}
        aria-label="graph name"
        onChange={(e) => store().setName(e.target.value)}
      />
      <span title={dirty ? 'unsaved changes' : 'saved'} className={`h-2 w-2 rounded-full ${dirty ? 'bg-[#f59e0b]' : 'bg-transparent'}`} />
      <div className="mx-2 h-5 w-px bg-[#3c3c3c]" />
      <button className={btnCls} onClick={() => void newGraphFlow()} title="New graph"><FilePlus size={14} />New</button>
      <button className={btnCls} onClick={() => void openFolderFlow()} title="Open graph folder"><FolderOpen size={14} />Open</button>
      <button className={btnCls} onClick={() => void store().save()} title="Save (Ctrl+S)"><Save size={14} />Save</button>
      <button className={btnCls} onClick={addNode} title="Add node (or double-click the canvas)"><Plus size={14} />Add node</button>
      <div className="mx-2 h-5 w-px bg-[#3c3c3c]" />
      <button className={btnCls} disabled={active} onClick={() => void store().startRun('all')}><Play size={14} />Run all</button>
      <button className={btnCls} disabled={active || selected === null} onClick={() => selected && void store().startRun({ from: selected })}>
        <PlayCircle size={14} />Run from selected
      </button>
      <button
        className={btnCls}
        disabled={active || !runHasFailures(runState)}
        onClick={() => runState && void store().startRun({ failed_of: runState.run_id })}
      >
        <RotateCcw size={14} />Rerun failed
      </button>
      {isRunActive(runState) && (
        <button className={`${btnCls} text-[#ef4444]`} onClick={() => void store().cancelRun()}><Square size={14} />Cancel</button>
      )}
      <div className="ml-auto flex items-center gap-2">
        {MOCK_MODE && <span className="rounded bg-[#f59e0b]/15 px-1.5 text-[11px] text-[#f59e0b]" title="No Tauri backend: in-memory mock">mock</span>}
        <ClaudeChip state={claude} />
        <button className={`${btnCls} ${logVisible ? 'bg-[#37373d]' : ''}`} onClick={onToggleLog} title="Show/hide the log panel">
          <PanelBottom size={14} />
        </button>
        <div className="relative">
          <button className={`${btnCls} ${settingsOpen ? 'bg-[#37373d]' : ''}`} onClick={() => setSettingsOpen((o) => !o)} title="Graph settings">
            <SettingsIcon size={14} />
          </button>
          {settingsOpen && <SettingsPopover />}
        </div>
      </div>
    </header>
  );
}

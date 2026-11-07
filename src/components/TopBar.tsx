/** Top bar: graph name, file actions, run controls, settings, claude status. */
import { useEffect, useRef, useState, type Dispatch, type RefObject, type SetStateAction } from 'react';
import { useReactFlow } from '@xyflow/react';
import {
  ExternalLink,
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
import { api, MOCK_MODE } from '../lib/api';
import { errorMessage } from '../store/runHelpers';
import { isRunActive, RERUN_FAILED_TITLE, runHasFailures } from '../lib/status';
import { graphStore, useGraphStore, type ClaudeState } from '../store/graphStore';
import { GRID } from './Canvas';
import { SessionChips } from './SessionChips';
import { SettingsPopover } from './SettingsPopover';
import { StatusPill } from './AssistantHeader';
import { showsPill } from '../lib/assistantView';
import { useAssistantStore } from '../store/assistantStore';
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

/** Popover open state that closes on Escape and on a mouse-down outside `ref`. */
function useDismissable(): [boolean, Dispatch<SetStateAction<boolean>>, RefObject<HTMLDivElement>] {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent): void => {
      if (ref.current && !ref.current.contains(e.target as globalThis.Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);
  return [open, setOpen, ref];
}

/** Assistant status while it works or waits for an answer; a click opens the Assistant tab. */
function AssistantChip({ onOpen }: { onOpen: () => void }) {
  const status = useAssistantStore((s) => s.state?.session.status ?? null);
  if (status === null || !showsPill(status)) return null;
  return (
    <button onClick={onOpen} title="Open the Assistant tab" aria-label="open the assistant tab">
      <StatusPill status={status} />
    </button>
  );
}

interface TopBarProps {
  logVisible: boolean;
  onToggleLog: () => void;
  onOpenAssistant: () => void;
}

export function TopBar({ logVisible, onToggleLog, onOpenAssistant }: TopBarProps) {
  const name = useGraphStore((s) => s.graph?.name ?? '');
  const dirty = useGraphStore((s) => s.dirty);
  const runState = useGraphStore((s) => s.runState);
  const selected = useGraphStore((s) => s.selectedNodeId);
  const claude = useGraphStore((s) => s.claude);
  const starting = useGraphStore((s) => s.startingRun);
  const dir = useGraphStore((s) => s.dir);
  const [settingsOpen, setSettingsOpen, settingsRef] = useDismissable();
  const { screenToFlowPosition } = useReactFlow();
  const store = graphStore().getState;
  const active = isRunActive(runState) || starting;

  const addNode = (): void => {
    const p = screenToFlowPosition({ x: window.innerWidth * 0.4, y: window.innerHeight * 0.4 });
    store().addNodeAt({ x: Math.round(p.x / GRID) * GRID, y: Math.round(p.y / GRID) * GRID });
  };

  const revealGraph = (): void => {
    if (!dir) return;
    api.revealGraphDir(dir).catch((err) => store().pushToast('error', `Could not open the folder: ${errorMessage(err)}`));
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
      <button className={btnCls} onClick={revealGraph} title="Show the graph folder in the file manager"><ExternalLink size={14} />Open folder</button>
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
        title={RERUN_FAILED_TITLE}
        onClick={() => runState && void store().startRun({ failed_of: runState.run_id })}
      >
        <RotateCcw size={14} />Rerun failed
      </button>
      {isRunActive(runState) && (
        <button className={`${btnCls} text-[#ef4444]`} onClick={() => void store().cancelRun()}><Square size={14} />Cancel</button>
      )}
      <SessionChips />
      <div className="ml-auto flex items-center gap-2">
        {MOCK_MODE && <span className="rounded bg-[#f59e0b]/15 px-1.5 text-[11px] text-[#f59e0b]" title="No Tauri backend: in-memory mock">mock</span>}
        <AssistantChip onOpen={onOpenAssistant} />
        <ClaudeChip state={claude} />
        <button className={`${btnCls} ${logVisible ? 'bg-[#37373d]' : ''}`} onClick={onToggleLog} title="Show/hide the log panel" aria-label="show or hide the log panel" aria-pressed={logVisible}>
          <PanelBottom size={14} />
        </button>
        <div className="relative" ref={settingsRef}>
          <button
            className={`${btnCls} ${settingsOpen ? 'bg-[#37373d]' : ''}`}
            onClick={() => setSettingsOpen((o) => !o)}
            title="Settings"
            aria-label="settings"
            aria-expanded={settingsOpen}
          >
            <SettingsIcon size={14} />
          </button>
          {settingsOpen && <SettingsPopover />}
        </div>
      </div>
    </header>
  );
}

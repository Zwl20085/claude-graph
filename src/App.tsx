/** App shell: start screen, or top bar + canvas + side panel (Assistant | Node) + log panel. */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import { isTauri } from './lib/api';
import { graphStore, useGraphStore } from './store/graphStore';
import { useStoredFlag } from './hooks/useStoredFlag';
import { Canvas } from './components/Canvas';
import { LogPanel } from './components/LogPanel';
import { SidePanel } from './components/SidePanel';
import { StartScreen } from './components/StartScreen';
import { Toasts } from './components/Toasts';
import { TopBar } from './components/TopBar';
import { RunSummaryStrip } from './components/RunSummaryStrip';
import { TrustDialog } from './components/TrustDialog';
import { PermissionDialog } from './components/PermissionDialog';
import { InboxDialog } from './components/InboxPanel';
import { errorMessage } from './store/runHelpers';
import { assistantStore } from './store/assistantStore';
import { ShortcutSheet, useShortcuts } from './components/Shortcuts';
import { SidePanelResizer, useSidePanelWidth } from './components/SidePanelResizer';
import { GRID } from './components/Canvas';
import type { Graph, Position } from './types/contract';

const APP_NAME = 'Claude Graph';

/** Window title "(N) Claude Graph — <name>*" (N: questions waiting); the native title too inside Tauri. */
export function windowTitle(name: string | null, dirty: boolean, questions: number): string {
  const base = name === null ? APP_NAME : `${APP_NAME} — ${name}${dirty ? '*' : ''}`;
  return questions > 0 ? `(${questions}) ${base}` : base;
}

function useWindowTitle(name: string | null, dirty: boolean, questions: number): void {
  useEffect(() => {
    const title = windowTitle(name, dirty, questions);
    document.title = title;
    if (!isTauri()) return;
    import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => getCurrentWindow().setTitle(title))
      .catch((err) => console.warn('setTitle failed (capability core:window:allow-set-title?)', err));
  }, [name, dirty, questions]);
}

/** Where Ctrl+N puts a node: one row below the lowest node (grid-snapped). */
export function nextNodePosition(graph: Graph | null): Position {
  const nodes = graph?.nodes ?? [];
  if (nodes.length === 0) return { x: 0, y: 0 };
  const lowest = nodes.reduce((a, b) => (b.position.y > a.position.y ? b : a));
  return { x: Math.round(lowest.position.x / GRID) * GRID, y: Math.round((lowest.position.y + 6 * GRID) / GRID) * GRID };
}

/**
 * Save pending edits before the window goes away.
 * Tauri: intercept the close request, await the save, then destroy the window
 * (capability core:window:allow-destroy); a failed save keeps the window open.
 * Browser (mock mode): best-effort save on beforeunload.
 */
function useSaveOnClose(): void {
  useEffect(() => {
    const toast = (text: string): void => graphStore().getState().pushToast('error', text);
    if (!isTauri()) {
      const onUnload = () => void graphStore().getState().saveAll();
      window.addEventListener('beforeunload', onUnload);
      return () => window.removeEventListener('beforeunload', onUnload);
    }
    let unlisten: (() => void) | null = null;
    let disposed = false;
    import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => {
        const win = getCurrentWindow();
        return win.onCloseRequested(async (event) => {
          event.preventDefault();
          if (!(await graphStore().getState().saveAll())) {
            toast('Not closing: the graph could not be saved. Fix the error or undo the change, then close again.');
            return;
          }
          await win.destroy().catch((err: unknown) => toast(`Could not close the window: ${errorMessage(err)}`));
        });
      })
      .then((off) => {
        if (disposed) off();
        else unlisten = off;
      })
      .catch((err: unknown) => toast(`Close handler not installed (unsaved edits may be lost on close): ${errorMessage(err)}`));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
}

/** Bind the assistant store to the open graph folder (unbinds on change and unmount). */
function useAssistantBinding(dir: string | null): void {
  useEffect(() => {
    const store = assistantStore().getState();
    if (dir === null) store.close();
    else void store.load(dir);
    return () => assistantStore().getState().close();
  }, [dir]);
}

/** A reveal request (badge, toast "Show") opens the side panel on the Node tab. */
function useReveal(onReveal: () => void): void {
  const seq = useGraphStore((s) => s.revealSeq);
  const last = useRef(seq);
  useEffect(() => {
    if (seq !== last.current) onReveal();
    last.current = seq;
  }, [seq, onReveal]);
}

/** A graph opened with no nodes starts on the Node tab (where the first node is made). */
function useNodeTabOnEmpty(dir: string | null, onEmpty: () => void): void {
  useEffect(() => {
    if (dir !== null && (graphStore().getState().graph?.nodes.length ?? 0) === 0) onEmpty();
  }, [dir, onEmpty]);
}

/** Selecting a node on the canvas switches the side panel to the Node tab. */
function useNodeTabOnSelect(onSelect: () => void): void {
  const selected = useGraphStore((s) => s.selectedNodeId);
  const last = useRef(selected);
  useEffect(() => {
    if (selected !== null && selected !== last.current) onSelect();
    last.current = selected;
  }, [selected, onSelect]);
}

export default function App() {
  const dir = useGraphStore((s) => s.dir);
  const name = useGraphStore((s) => s.graph?.name ?? null);
  const dirty = useGraphStore((s) => s.dirty);
  const [logVisible, setLogVisible] = useStoredFlag('cg.logVisible', true);
  const [inspectorOpen, setInspectorOpen] = useStoredFlag('cg.inspectorOpen', true);
  const [assistantTab, setAssistantTab] = useStoredFlag('cg.sideTab.assistant', true);
  const showNodeTab = useCallback(() => setAssistantTab(false), [setAssistantTab]);
  const revealPanel = useCallback(() => {
    setInspectorOpen(true);
    setAssistantTab(false);
  }, [setInspectorOpen, setAssistantTab]);
  const questions = useGraphStore((s) => s.sessionPrompts.length);
  const [sheet, setSheet] = useState(false);
  const [panelWidth, setPanelWidth] = useSidePanelWidth();
  const shortcuts = useMemo(
    () => ({
      addNode: () => void graphStore().getState().addNodeAt(nextNodePosition(graphStore().getState().graph)),
      toggleLog: () => setLogVisible(!logVisible),
      showSheet: () => setSheet(true),
    }),
    [logVisible, setLogVisible],
  );
  const openAssistant = (): void => {
    setInspectorOpen(true);
    setAssistantTab(true);
  };
  useAssistantBinding(dir);
  useNodeTabOnSelect(showNodeTab);
  useReveal(revealPanel);
  useNodeTabOnEmpty(dir, showNodeTab);
  useWindowTitle(name, dirty, questions);
  useShortcuts(shortcuts);
  useSaveOnClose();

  return (
    <div className="flex h-screen flex-col bg-[#1e1e1e] text-[#d4d4d4]">
      {dir === null ? (
        <StartScreen />
      ) : (
        <ReactFlowProvider key={dir}>
          <TopBar logVisible={logVisible} onToggleLog={() => setLogVisible(!logVisible)} onOpenAssistant={openAssistant} />
          <RunSummaryStrip />
          <div className="flex min-h-0 flex-1">
            <div className="flex min-w-0 flex-1 flex-col">
              <main className="min-h-0 flex-1">
                <Canvas />
              </main>
              {logVisible && (
                <section className="h-[220px] shrink-0 border-t border-[#3c3c3c] bg-[#252526]">
                  <LogPanel scheduler={inspectorOpen && assistantTab} />
                </section>
              )}
            </div>
            <button
              className="w-4 shrink-0 border-l border-[#3c3c3c] bg-[#252526] text-[#9a9a9a] hover:text-white"
              onClick={() => setInspectorOpen(!inspectorOpen)}
              title={inspectorOpen ? 'Collapse side panel' : 'Expand side panel'}
              aria-label={inspectorOpen ? 'collapse side panel' : 'expand side panel'}
              aria-expanded={inspectorOpen}
            >
              {inspectorOpen ? <ChevronRight size={12} /> : <ChevronLeft size={12} />}
            </button>
            {inspectorOpen && (
              <aside className="relative shrink-0 bg-[#252526]" style={{ width: panelWidth }}>
                <SidePanelResizer width={panelWidth} onWidth={setPanelWidth} />
                <SidePanel assistant={assistantTab} onSelectTab={setAssistantTab} />
              </aside>
            )}
          </div>
        </ReactFlowProvider>
      )}
      {sheet && <ShortcutSheet onClose={() => setSheet(false)} />}
      <TrustDialog />
      <PermissionDialog />
      <InboxDialog />
      <Toasts />
    </div>
  );
}

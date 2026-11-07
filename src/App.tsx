/** App shell: start screen, or top bar + canvas + side panel (Assistant | Node) + log panel. */
import { useCallback, useEffect, useRef } from 'react';
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
import { TrustDialog } from './components/TrustDialog';
import { PermissionDialog } from './components/PermissionDialog';
import { errorMessage } from './store/runHelpers';
import { assistantStore } from './store/assistantStore';

const APP_NAME = 'Claude Graph';

/** Window title "Claude Graph — <name>*"; the native title too when inside Tauri. */
function useWindowTitle(name: string | null, dirty: boolean): void {
  useEffect(() => {
    const title = name === null ? APP_NAME : `${APP_NAME} — ${name}${dirty ? '*' : ''}`;
    document.title = title;
    if (!isTauri()) return;
    import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => getCurrentWindow().setTitle(title))
      .catch((err) => console.warn('setTitle failed (capability core:window:allow-set-title?)', err));
  }, [name, dirty]);
}

function useSaveShortcut(): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
        e.preventDefault();
        void graphStore().getState().save();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
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
  const openAssistant = (): void => {
    setInspectorOpen(true);
    setAssistantTab(true);
  };
  useAssistantBinding(dir);
  useNodeTabOnSelect(showNodeTab);
  useWindowTitle(name, dirty);
  useSaveShortcut();
  useSaveOnClose();

  return (
    <div className="flex h-screen flex-col bg-[#1e1e1e] text-[#d4d4d4]">
      {dir === null ? (
        <StartScreen />
      ) : (
        <ReactFlowProvider key={dir}>
          <TopBar logVisible={logVisible} onToggleLog={() => setLogVisible(!logVisible)} onOpenAssistant={openAssistant} />
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
              <aside className="w-[400px] shrink-0 bg-[#252526]">
                <SidePanel assistant={assistantTab} onSelectTab={setAssistantTab} />
              </aside>
            )}
          </div>
        </ReactFlowProvider>
      )}
      <TrustDialog />
      <PermissionDialog />
      <Toasts />
    </div>
  );
}

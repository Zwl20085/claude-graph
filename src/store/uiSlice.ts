/**
 * UI slice of the graph store: which Inspector tab is shown (kept across node
 * changes), requests to reveal a node (select it, open the side panel on a
 * tab), the notification history behind the bell, the lines sent to each
 * node in each run, and the runs whose "skip this question" was confirmed.
 * Every update builds new objects.
 */

export const INSPECTOR_TABS = ['prompt', 'brief', 'chat', 'results'] as const;
export type InspectorTab = (typeof INSPECTOR_TABS)[number];

/** Notifications kept for the bell. */
export const HISTORY_CAP = 20;

export interface UiState {
  inspectorTab: InspectorTab;
  /** Bumped by `revealNode`: the app opens the side panel on the Node tab. */
  revealSeq: number;
  /** Lines typed to a node, keyed `<run>:<node>`. */
  sentLines: Readonly<Record<string, readonly string[]>>;
  /** Runs in which the user confirmed "skip this question" once. */
  skipConfirmedRuns: readonly string[];
}

export interface UiActions {
  setInspectorTab: (tab: InspectorTab) => void;
  /** Select `nodeId`, show its `tab` (default: keep the current tab), open the side panel. */
  revealNode: (nodeId: string, tab?: InspectorTab) => void;
  addSentLine: (runId: string, nodeId: string, line: string) => void;
  confirmSkip: (runId: string) => void;
}

export const INITIAL_UI: UiState = { inspectorTab: 'prompt', revealSeq: 0, sentLines: {}, skipConfirmedRuns: [] };

export const sentKey = (runId: string, nodeId: string): string => `${runId}:${nodeId}`;

type Get = () => UiState;
type SetPatch = (patch: Partial<UiState> & { selectedNodeId?: string | null }) => void;

export function createUiActions(set: SetPatch, get: Get): UiActions {
  return {
    setInspectorTab: (inspectorTab) => set({ inspectorTab }),
    revealNode: (nodeId, tab) =>
      set({ selectedNodeId: nodeId, inspectorTab: tab ?? get().inspectorTab, revealSeq: get().revealSeq + 1 }),
    addSentLine: (runId, nodeId, line) => {
      const key = sentKey(runId, nodeId);
      const lines = get().sentLines;
      set({ sentLines: { ...lines, [key]: [...(lines[key] ?? []), line] } });
    },
    confirmSkip: (runId) => {
      const runs = get().skipConfirmedRuns;
      if (!runs.includes(runId)) set({ skipConfirmedRuns: [...runs, runId] });
    },
  };
}

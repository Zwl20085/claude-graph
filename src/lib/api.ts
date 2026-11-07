/**
 * The one IPC surface the UI talks to. Inside Tauri it is `./ipc` (real
 * commands); in a plain browser (`npm run dev` without Tauri) it is the
 * in-memory mock from `./ipc.mock`. Selection: `window.__TAURI_INTERNALS__`.
 */
import type {
  AppSettings,
  AssistantState,
  ClaudeCheck,
  Graph,
  LaunchOptions,
  NodeEvent,
  NodeMessage,
  NodeResult,
  RunState,
  RunSummary,
  RunTarget,
  TrustStatus,
} from '../types/contract';
import * as tauriIpc from './ipc';
import { createMockIpc } from './ipc.mock';

export type Unlisten = () => void;

export interface Api {
  checkClaude: () => Promise<ClaudeCheck>;
  getAppSettings: () => Promise<AppSettings>;
  setAppSettings: (settings: AppSettings) => Promise<AppSettings>;
  graphTrustStatus: (dir: string) => Promise<TrustStatus>;
  trustGraph: (dir: string) => Promise<void>;
  createGraph: (dir: string, name: string) => Promise<Graph>;
  openGraph: (dir: string) => Promise<Graph>;
  saveGraph: (dir: string, graph: Graph) => Promise<void>;
  listRecent: () => Promise<string[]>;
  pickFolder: (title: string) => Promise<string | null>;
  startRun: (dir: string, target: RunTarget) => Promise<string>;
  cancelRun: (run_id: string) => Promise<void>;
  cancelNode: (run_id: string, node_id: string) => Promise<void>;
  getRun: (dir: string, run_id: string) => Promise<RunState>;
  listRuns: (dir: string) => Promise<RunSummary[]>;
  readNodeResult: (dir: string, run_id: string, node_id: string) => Promise<NodeResult>;
  renderBrief: (graph: Graph, node_id: string) => Promise<string>;
  listMessages: (dir: string, run_id: string, node_id: string) => Promise<NodeMessage[]>;
  sendMessage: (dir: string, run_id: string, node_id: string, text: string) => Promise<void>;
  revealGraphDir: (dir: string) => Promise<void>;
  revealNodeDir: (dir: string, node_id: string) => Promise<void>;
  launchOptions: () => Promise<LaunchOptions>;
  onRunState: (handler: (state: RunState) => void) => Promise<Unlisten>;
  onNodeEvent: (handler: (event: NodeEvent) => void) => Promise<Unlisten>;
  assistantState: (dir: string) => Promise<AssistantState>;
  assistantSend: (dir: string, text: string) => Promise<AssistantState>;
  assistantCancel: (dir: string) => Promise<void>;
  assistantReset: (dir: string) => Promise<AssistantState>;
  onAssistantUpdate: (handler: (state: AssistantState) => void) => Promise<Unlisten>;
}

/** True when running inside a Tauri WebView. */
export function isTauri(win: unknown = globalThis): boolean {
  return typeof win === 'object' && win !== null && '__TAURI_INTERNALS__' in win;
}

export function selectApi(win: unknown = globalThis): { api: Api; mock: boolean } {
  if (isTauri(win)) return { api: tauriIpc, mock: false };
  return { api: createMockIpc(), mock: true };
}

const selected = selectApi();

export const api: Api = selected.api;
/** True when the in-memory mock backend is in use (shown in the top bar). */
export const MOCK_MODE: boolean = selected.mock;

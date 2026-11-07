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
  HarnessCheck,
  HarnessProfile,
  LaunchOptions,
  NodeEvent,
  NodeMessage,
  NodeResult,
  PermissionAnswer,
  PermissionAnswered,
  PermissionPrompt,
  PtyData,
  PtyReplay,
  RunState,
  RunSummary,
  RunTarget,
  TerminalSession,
  TrustStatus,
} from '../types/contract';
import * as tauriIpc from './ipc';
import { createMockIpc } from './ipc.mock';

export type Unlisten = () => void;

export interface Api {
  checkClaude: () => Promise<ClaudeCheck>;
  getAppSettings: () => Promise<AppSettings>;
  setAppSettings: (settings: AppSettings) => Promise<AppSettings>;
  // Harness profiles (HARNESS_CONTRACT): machine-local, built-ins first.
  listHarnesses: () => Promise<HarnessProfile[]>;
  saveHarness: (profile: HarnessProfile) => Promise<HarnessProfile[]>;
  deleteHarness: (id: string) => Promise<HarnessProfile[]>;
  checkHarness: (id: string) => Promise<HarnessCheck>;
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
  // Terminal mode (TERMINAL_CONTRACT "Commands and events").
  listSessions: (dir: string, run_id: string) => Promise<TerminalSession[]>;
  ptyWrite: (run_id: string, sid: string, text: string) => Promise<void>;
  ptyResize: (run_id: string, sid: string, cols: number, rows: number) => Promise<void>;
  ptyReplay: (run_id: string, sid: string) => Promise<PtyReplay>;
  openSessionWindow: (run_id: string, sid: string) => Promise<void>;
  onPtyData: (handler: (data: PtyData) => void) => Promise<Unlisten>;
  // "Ask me in the app" (ASKAPP_CONTRACT).
  listPermissionPrompts: () => Promise<PermissionPrompt[]>;
  answerPermission: (answer: PermissionAnswer) => Promise<void>;
  onPermissionPrompt: (handler: (prompt: PermissionPrompt) => void) => Promise<Unlisten>;
  onPermissionAnswered: (handler: (answered: PermissionAnswered) => void) => Promise<Unlisten>;
}

/** True when running inside a Tauri WebView. */
export function isTauri(win: unknown = globalThis): boolean {
  return typeof win === 'object' && win !== null && '__TAURI_INTERNALS__' in win;
}

export function selectApi(win: unknown = globalThis): { api: Api; mock: boolean } {
  if (isTauri(win)) return { api: tauriIpc, mock: false };
  // Browser dev: chips open a second tab, and tabs share pty output over a BroadcastChannel.
  const open = (globalThis as { open?: (url: string, target: string) => unknown }).open;
  return {
    api: createMockIpc({
      broadcast: true,
      exampleRunMode: 'as_file',
      openWindow: typeof open === 'function' ? (url, target) => void open.call(globalThis, url, target) : undefined,
    }),
    mock: true,
  };
}

const selected = selectApi();

export const api: Api = selected.api;
/** True when the in-memory mock backend is in use (shown in the top bar). */
export const MOCK_MODE: boolean = selected.mock;

/**
 * Typed wrappers for every Tauri command and event in docs/ARCHITECTURE.md §6.
 *
 * Argument names are snake_case: the Rust commands are declared with
 * `#[tauri::command(rename_all = "snake_case")]`.
 * Every promise rejects with an `AppError` ({ code, message }).
 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
  EVENT_ASSISTANT_UPDATE,
  EVENT_NODE_EVENT,
  EVENT_PERMISSION_ANSWERED,
  EVENT_PERMISSION_PROMPT,
  EVENT_PTY_DATA,
  EVENT_RUN_STATE,
  isAppError,
  type AppError,
  type AppSettings,
  type AssistantState,
  type ClaudeCheck,
  type Graph,
  type HarnessCheck,
  type HarnessProfile,
  type LaunchOptions,
  type NodeEvent,
  type NodeMessage,
  type NodeResult,
  type PermissionAnswer,
  type PermissionAnswered,
  type PermissionPrompt,
  type PtyData,
  type PtyReplay,
  type RunState,
  type RunSummary,
  type RunTarget,
  type TerminalSession,
  type TrustStatus,
} from '../types/contract';

/** Readable text of a non-AppError rejection (Error message, JSON, or String fallback). */
export function describeError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === 'string') return err;
  try {
    const json = JSON.stringify(err);
    if (json !== undefined) return json;
  } catch {
    // circular or otherwise unserialisable: fall through
  }
  return String(err);
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (err) {
    if (isAppError(err)) throw err;
    const wrapped: AppError = { code: 'ipc', message: describeError(err) };
    throw wrapped;
  }
}

/** Resolution order on the backend: app settings -> env CLAUDE_GRAPH_CLAUDE_BIN -> PATH. */
export const checkClaude = (): Promise<ClaudeCheck> => call('check_claude');

export const getAppSettings = (): Promise<AppSettings> => call('get_app_settings');

/** Validated by the backend (absolute local path to an existing claude file); rejects with invalid_input. */
export const setAppSettings = (settings: AppSettings): Promise<AppSettings> => call('set_app_settings', { settings });

/** Built-ins (with this machine's overrides), then custom profiles. */
export const listHarnesses = (): Promise<HarnessProfile[]> => call('list_harnesses');

/** Create or update a profile (a built-in id stores an override); rejects with invalid_input. Returns the new list. */
export const saveHarness = (profile: HarnessProfile): Promise<HarnessProfile[]> => call('save_harness', { profile });

/** Delete a custom profile (built-ins reject with invalid_input). Returns the new list. */
export const deleteHarness = (id: string): Promise<HarnessProfile[]> => call('delete_harness', { id });

/** Runs `<command> --version` with the profile env (10 s timeout). */
export const checkHarness = (id: string): Promise<HarnessCheck> => call('check_harness', { id });

export const graphTrustStatus = (dir: string): Promise<TrustStatus> => call('graph_trust_status', { dir });

export const trustGraph = (dir: string): Promise<void> => call('trust_graph', { dir });

export const createGraph = (dir: string, name: string): Promise<Graph> =>
  call('create_graph', { dir, name });

export const openGraph = (dir: string): Promise<Graph> => call('open_graph', { dir });

export const saveGraph = (dir: string, graph: Graph): Promise<void> =>
  call('save_graph', { dir, graph });

export const listRecent = (): Promise<string[]> => call('list_recent');

export const pickFolder = (title: string): Promise<string | null> =>
  call('pick_folder', { title });

export const startRun = (dir: string, target: RunTarget): Promise<string> =>
  call('start_run', { dir, target });

export const cancelRun = (run_id: string): Promise<void> => call('cancel_run', { run_id });

export const cancelNode = (run_id: string, node_id: string): Promise<void> =>
  call('cancel_node', { run_id, node_id });

export const getRun = (dir: string, run_id: string): Promise<RunState> =>
  call('get_run', { dir, run_id });

export const listRuns = (dir: string): Promise<RunSummary[]> => call('list_runs', { dir });

export const readNodeResult = (
  dir: string,
  run_id: string,
  node_id: string,
): Promise<NodeResult> => call('read_node_result', { dir, run_id, node_id });

export const renderBrief = (graph: Graph, node_id: string): Promise<string> =>
  call('render_brief', { graph, node_id });

/** Chat history of one node session in a run, oldest first. */
export const listMessages = (dir: string, run_id: string, node_id: string): Promise<NodeMessage[]> =>
  call('list_messages', { dir, run_id, node_id });

/** Send a follow-up message to a node session (queued if running, resumes it if finished). */
export const sendMessage = (dir: string, run_id: string, node_id: string, text: string): Promise<void> =>
  call('send_message', { dir, run_id, node_id, text });

/** Reveal graph.json of the graph folder in the OS file manager. */
export const revealGraphDir = (dir: string): Promise<void> => call('reveal_graph_dir', { dir });

/** Reveal `nodes/<node_id>/deliverables` in the OS file manager. */
export const revealNodeDir = (dir: string, node_id: string): Promise<void> => call('reveal_node_dir', { dir, node_id });

/** Startup request from the command line / env (see ARCHITECTURE.md "Launching"). */
export const launchOptions = (): Promise<LaunchOptions> => call('launch_options');

/** Subscribe to `run:state`. Resolves to an unsubscribe function. */
export const onRunState = (handler: (state: RunState) => void): Promise<UnlistenFn> =>
  listen<RunState>(EVENT_RUN_STATE, (e) => handler(e.payload));

/** Subscribe to `node:event`. Resolves to an unsubscribe function. */
export const onNodeEvent = (handler: (event: NodeEvent) => void): Promise<UnlistenFn> =>
  listen<NodeEvent>(EVENT_NODE_EVENT, (e) => handler(e.payload));

/** Assistant journal + session of the graph folder (empty idle state when assistant/ is missing). */
export const assistantState = (dir: string): Promise<AssistantState> => call('assistant_state', { dir });

/** Append a user message (instruction, or answer when waiting) and start a scheduler turn. */
export const assistantSend = (dir: string, text: string): Promise<AssistantState> =>
  call('assistant_send', { dir, text });

/** Kill the running scheduler turn; rejects with not_found when nothing runs. */
export const assistantCancel = (dir: string): Promise<void> => call('assistant_cancel', { dir });

/** Forget the scheduler session (journal kept); rejects with assistant_busy while working. */
export const assistantReset = (dir: string): Promise<AssistantState> => call('assistant_reset', { dir });

/** Subscribe to `assistant_update`. The payload names no folder: callers reload their own. */
export const onAssistantUpdate = (handler: (state: AssistantState) => void): Promise<UnlistenFn> =>
  listen<AssistantState>(EVENT_ASSISTANT_UPDATE, (e) => handler(e.payload));

// ---------------------------------------------------------------------------
// Terminal mode
// ---------------------------------------------------------------------------

/** Sessions of a terminal-mode run (empty for a headless run). */
export const listSessions = (dir: string, run_id: string): Promise<TerminalSession[]> =>
  call('list_sessions', { dir, run_id });

/** Raw text to the session's pty (keystrokes, or a message followed by "
"). */
export const ptyWrite = (run_id: string, sid: string, text: string): Promise<void> =>
  call('pty_write', { run_id, sid, text });

export const ptyResize = (run_id: string, sid: string, cols: number, rows: number): Promise<void> =>
  call('pty_resize', { run_id, sid, cols, rows });

/**
 * The session transcript so far (last 512 KiB), for a window opened late, with
 * the seq of its last chunk: live `pty:data` chunks with seq <= that are in it.
 */
export const ptyReplay = (run_id: string, sid: string): Promise<PtyReplay> => call('pty_replay', { run_id, sid });

/** Create or focus the window `term-<run>-<sid>`. */
export const openSessionWindow = (run_id: string, sid: string): Promise<void> =>
  call('open_session_window', { run_id, sid });

/** Subscribe to `pty:data`; callers filter on run_id / sid. */
export const onPtyData = (handler: (data: PtyData) => void): Promise<UnlistenFn> =>
  listen<PtyData>(EVENT_PTY_DATA, (e) => handler(e.payload));

// ---------------------------------------------------------------------------
// "Ask me in the app" permission prompts (ASKAPP_CONTRACT)
// ---------------------------------------------------------------------------

/** Pending permission prompts of the app, oldest first. */
export const listPermissionPrompts = (): Promise<PermissionPrompt[]> => call('list_permission_prompts');

/** Answer one prompt; rejects with not_found when it was already answered. */
export const answerPermission = (answer: PermissionAnswer): Promise<void> => call('answer_permission', { answer });

/** Subscribe to `permission_prompt`. */
export const onPermissionPrompt = (handler: (prompt: PermissionPrompt) => void): Promise<UnlistenFn> =>
  listen<PermissionPrompt>(EVENT_PERMISSION_PROMPT, (e) => handler(e.payload));

/** Subscribe to `permission_answered`. */
export const onPermissionAnswered = (handler: (answered: PermissionAnswered) => void): Promise<UnlistenFn> =>
  listen<PermissionAnswered>(EVENT_PERMISSION_ANSWERED, (e) => handler(e.payload));

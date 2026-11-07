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
  EVENT_NODE_EVENT,
  EVENT_RUN_STATE,
  isAppError,
  type AppError,
  type ClaudeCheck,
  type Graph,
  type NodeEvent,
  type NodeMessage,
  type NodeResult,
  type RunState,
  type RunSummary,
  type RunTarget,
} from '../types/contract';

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (err) {
    if (isAppError(err)) throw err;
    const wrapped: AppError = { code: 'ipc', message: String(err) };
    throw wrapped;
  }
}

/** `claude_bin` overrides PATH lookup (pass settings.claude_bin); may be omitted. */
export const checkClaude = (claude_bin?: string | null): Promise<ClaudeCheck> =>
  call('check_claude', { claude_bin: claude_bin ?? null });

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

export const openPath = (path: string): Promise<void> => call('open_path', { path });

/** Subscribe to `run:state`. Resolves to an unsubscribe function. */
export const onRunState = (handler: (state: RunState) => void): Promise<UnlistenFn> =>
  listen<RunState>(EVENT_RUN_STATE, (e) => handler(e.payload));

/** Subscribe to `node:event`. Resolves to an unsubscribe function. */
export const onNodeEvent = (handler: (event: NodeEvent) => void): Promise<UnlistenFn> =>
  listen<NodeEvent>(EVENT_NODE_EVENT, (e) => handler(e.payload));

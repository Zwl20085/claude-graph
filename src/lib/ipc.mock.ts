/**
 * In-memory stand-in for the Tauri backend, used when the UI runs in a plain
 * browser. Graphs live in a Map keyed by folder; a run is simulated with
 * timers that emit the same `run:state` / `node:event` sequence the real
 * scheduler would (pending -> queued -> running -> succeeded|failed, failure
 * skips dependents, max_parallel respected). A node whose extra_context
 * contains FAKE_FAIL fails, like scripts/fake-claude.mjs; a prompt with
 * FAKE_BLOCKED ends blocked and one with FAKE_DENY reports a denied tool call.
 * FAKE_ASK=<tool> (headless) asks for approval of one call and waits for
 * `answer_permission` (see ./mockPermissions).
 */
import example from '../../examples/hello-pipeline/graph.json';
import type {
  AppError,
  AppSettings,
  AssistantState,
  ClaudeCheck,
  Graph,
  LaunchOptions,
  NodeEvent,
  NodeEventKind,
  NodeResult,
  PermissionAnswered,
  PermissionPrompt,
  PtyData,
  RunMode,
  RunState,
  RunSummary,
  RunTarget,
  SessionPrompt,
  SessionPromptCleared,
} from '../types/contract';
import type { Api, Unlisten } from './api';
import { renderBrief } from './brief';
import { clampInbox } from './inboxSettings';
import { defaultTimers, type Timers } from './debounce';
import { emptyGraph } from './graphOps';
import { createMockAssistant, MOCK_ASSISTANT_REPLY_MS } from './mockAssistant';
import { createMockHarnesses } from './mockHarness';
import { createMockPermissions } from './mockPermissions';
import { createMockPrompts } from './mockPrompts';
import { createMockRunner, targetSet, type MockRun } from './mockRunner';
import { createMockTerminal, MOCK_PTY_REPLY_MS, type MockChannel } from './mockTerminal';
import { countStatuses } from './status';
import { trustFingerprint, trustItems } from './trust';
import { claudeBinError, validateGraph } from './validate';

export const MOCK_EXAMPLE_DIR = '/mock/hello-pipeline';

export interface MockOptions {
  timers?: Timers;
  /** Delay between simulated stream events of a node. */
  stepMs?: number;
  now?: () => Date;
  /** Seed the example graph (default true). */
  seed?: boolean;
  /** Folder answer for pick_folder (default: window.prompt). */
  pickFolder?: (title: string) => string | null;
  /** Treat every graph as trusted (skips the trust dialog; default false). */
  trustAll?: boolean;
  /** Delay of the mock scheduler's reply in assistant mode. */
  assistantReplyMs?: number;
  /**
   * run_mode written into the seeded example. Default 'headless': the
   * existing mock tests exercise the headless engine. 'as_file' keeps the
   * file as it is (no run_mode = terminal); browser dev uses that.
   */
  exampleRunMode?: RunMode | 'as_file';
  /** Delay of the fake reply to a line typed into a mock terminal. */
  ptyReplyMs?: number;
  /** Open a session "window" (browser dev: a new tab). Default: nothing. */
  openWindow?: (url: string, target: string) => void;
  /** Share terminal sessions with other tabs over BroadcastChannel (browser dev). */
  broadcast?: boolean;
  /** Injected channel (tests); wins over `broadcast`. */
  channel?: MockChannel;
  /**
   * Terminal runs show the turn outcomes (TURN_CONTRACT): a choice dialog on
   * the first node, a reply question after the second (browser dev).
   */
  simulatePrompts?: boolean;
}

const MOCK_CHANNEL_NAME = 'claude-graph-mock';

function openChannel(options: MockOptions): MockChannel | null {
  if (options.channel) return options.channel;
  if (options.broadcast !== true) return null;
  const BC = (globalThis as { BroadcastChannel?: new (name: string) => MockChannel }).BroadcastChannel;
  return typeof BC === 'function' ? new BC(MOCK_CHANNEL_NAME) : null;
}

const appError = (code: string, message: string): AppError => ({ code, message });
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

function promptFolder(title: string): string | null {
  const p = (globalThis as { prompt?: (msg: string, def?: string) => string | null }).prompt;
  if (typeof p !== 'function') return null;
  const answer = p(`${title}\n(mock mode: type any folder path)`, '/mock/new-graph');
  return answer === null || answer.trim() === '' ? null : answer.trim();
}

/** `window.__CG_LAUNCH__` stands in for argv/env in mock mode (tests, browser dev). */
function mockLaunchOptions(): LaunchOptions {
  const raw = (globalThis as { __CG_LAUNCH__?: Partial<LaunchOptions> }).__CG_LAUNCH__;
  const dir = typeof raw?.open_dir === 'string' && raw.open_dir.trim() !== '' ? raw.open_dir.trim() : null;
  return { open_dir: dir, autorun: dir !== null && raw?.autorun === true };
}

function summarise(run: MockRun): RunSummary {
  const costs = Object.values(run.state.nodes)
    .map((n) => n.cost_usd)
    .filter((c): c is number => c !== null);
  return {
    run_id: run.state.run_id,
    started_at: run.state.started_at,
    finished_at: run.state.finished_at,
    status: run.state.status,
    counts: countStatuses(run.state),
    total_cost_usd: costs.length === 0 ? null : costs.reduce((a, b) => a + b, 0),
  };
}

export function createMockIpc(options: MockOptions = {}): Api {
  const graphs = new Map<string, Graph>();
  let recent: string[] = [];
  const stateListeners = new Set<(s: RunState) => void>();
  const eventListeners = new Set<(e: NodeEvent) => void>();
  const assistantListeners = new Set<(s: AssistantState) => void>();
  const ptyListeners = new Set<(d: PtyData) => void>();
  const promptListeners = new Set<(p: PermissionPrompt) => void>();
  const answeredListeners = new Set<(a: PermissionAnswered) => void>();
  const sessionPromptListeners = new Set<(p: SessionPrompt) => void>();
  const clearedListeners = new Set<(c: SessionPromptCleared) => void>();
  /** dir -> accepted fingerprint of the elevated fields (stand-in for trusted.json). */
  const trusted = new Map<string, string>();
  let appSettings: AppSettings = { claude_bin: null };
  const harnesses = createMockHarnesses();

  const trustStatus = (dir: string) => {
    const items = trustItems(requireGraph(dir));
    const ok = options.trustAll === true || items.length === 0 || trusted.get(dir) === trustFingerprint(items);
    return { trusted: ok, items };
  };

  const touchRecent = (dir: string): void => {
    recent = [dir, ...recent.filter((d) => d !== dir)].slice(0, 20);
  };

  const terminal = createMockTerminal({
    timers: options.timers ?? defaultTimers,
    replyMs: options.ptyReplyMs ?? MOCK_PTY_REPLY_MS,
    getRun: (runId) => runner.get(runId),
    emitPty: (d) => ptyListeners.forEach((h) => h({ ...d })),
    emitRemoteState: (s) => stateListeners.forEach((h) => h(clone(s))),
    openWindow: options.openWindow,
    channel: openChannel(options),
  });

  const nodeEvent = (runId: string, nodeId: string, kind: NodeEventKind, text: string): void => {
    const ev: NodeEvent = { run_id: runId, node_id: nodeId, kind, text, ts: (options.now ?? (() => new Date()))().toISOString() };
    eventListeners.forEach((h) => h(ev));
    terminal.onNodeEvent(runId, nodeId, kind, text);
  };

  const permissions = createMockPermissions({
    now: options.now ?? (() => new Date()),
    emitPrompt: (p) => promptListeners.forEach((h) => h(clone(p))),
    emitAnswered: (id) => answeredListeners.forEach((h) => h({ id })),
    emitEvent: nodeEvent,
  });

  const prompts = createMockPrompts({
    emitPrompt: (p) => sessionPromptListeners.forEach((h) => h(clone(p))),
    emitCleared: (c) => clearedListeners.forEach((h) => h(clone(c))),
    emitEvent: nodeEvent,
  });

  const runner = createMockRunner({
    timers: options.timers ?? defaultTimers,
    stepMs: options.stepMs ?? 600,
    now: options.now ?? (() => new Date()),
    harnessLabel: (id) => harnesses.label(id),
    emitState: (s) => {
      stateListeners.forEach((h) => h(clone(s)));
      terminal.shareState(clone(s));
    },
    emitEvent: (dir, runId, nodeId, kind: NodeEventKind, text) => {
      void dir;
      nodeEvent(runId, nodeId, kind, text);
    },
    permissions,
    prompts: options.simulatePrompts === true ? prompts.hooks : undefined,
  });

  /** Re-emit a run after a prompt changed (session pending_prompt, node status). */
  const refresh = (runId: string): void => {
    const run = runner.get(runId);
    if (run) stateListeners.forEach((h) => h(clone(run.state)));
  };

  const assistant = createMockAssistant({
    timers: options.timers ?? defaultTimers,
    now: options.now ?? (() => new Date()),
    replyMs: options.assistantReplyMs ?? MOCK_ASSISTANT_REPLY_MS,
    graphOf: (dir) => requireGraph(dir),
    emitUpdate: (s) => assistantListeners.forEach((h) => h(clone(s))),
    emitEvent: (runId, nodeId, kind, text) => {
      const ev: NodeEvent = { run_id: runId, node_id: nodeId, kind, text, ts: (options.now ?? (() => new Date()))().toISOString() };
      eventListeners.forEach((h) => h(ev));
    },
  });

  if (options.seed !== false) {
    const g = validateGraph(example);
    if (!(g instanceof Error)) {
      const mode = options.exampleRunMode ?? 'headless';
      graphs.set(MOCK_EXAMPLE_DIR, mode === 'as_file' ? g : { ...g, settings: { ...g.settings, run_mode: mode } });
      touchRecent(MOCK_EXAMPLE_DIR);
    }
  }

  const requireGraph = (dir: string): Graph => {
    const g = graphs.get(dir);
    if (!g) throw appError('not_found', `no graph.json in ${dir}`);
    return g;
  };

  /** Target ids, or [] when the target cannot be resolved (runner.start then reports why). */
  const knownTargets = (graph: Graph, target: RunTarget): string[] => {
    try {
      return targetSet(graph, target, (id) => runner.get(id)?.state);
    } catch {
      return [];
    }
  };

  const requireRun = (dir: string, runId: string): MockRun => {
    const run = runner.get(runId);
    if (!run || run.state.graph_dir !== dir) throw appError('not_found', `run ${runId} not found in ${dir}`);
    return run;
  };

  return {
    checkClaude: async (): Promise<ClaudeCheck> => ({
      found: true,
      path: appSettings.claude_bin ?? '/mock/claude',
      version: '2.1.0 (mock)',
    }),
    getAppSettings: async () => clone(appSettings),
    setAppSettings: async (settings) => {
      const bin = settings.claude_bin === null || settings.claude_bin.trim() === '' ? null : settings.claude_bin.trim();
      const err = bin === null ? null : claudeBinError(bin);
      if (err) throw appError('invalid_input', err);
      const inbox = settings.inbox ? clampInbox(settings.inbox) : undefined;
      if (inbox?.enabled && inbox.folder === '') throw appError('invalid_input', 'choose the tips folder before enabling the inbox');
      // Like serde defaults on the backend: fields not sent are reset (callers send the whole object).
      appSettings = {
        claude_bin: bin,
        ...(inbox ? { inbox } : {}),
        ...(settings.autostart !== undefined ? { autostart: settings.autostart } : {}),
      };
      return clone(appSettings);
    },
    listHarnesses: async () => harnesses.list(),
    saveHarness: async (profile) => harnesses.save(clone(profile)),
    deleteHarness: async (id) => harnesses.remove(id),
    checkHarness: async (id) => harnesses.check(id),
    graphTrustStatus: async (dir) => clone(trustStatus(dir)),
    trustGraph: async (dir) => {
      trusted.set(dir, trustFingerprint(trustStatus(dir).items));
    },
    createGraph: async (dir, name) => {
      if (graphs.has(dir)) throw appError('io', `graph.json already exists in ${dir}`);
      const g = emptyGraph(name);
      graphs.set(dir, g);
      touchRecent(dir);
      return clone(g);
    },
    openGraph: async (dir) => {
      const g = requireGraph(dir);
      touchRecent(dir);
      return clone(g);
    },
    saveGraph: async (dir, graph) => {
      const g = validateGraph(clone(graph));
      if (g instanceof Error) throw appError('invalid_graph', g.message);
      graphs.set(dir, g);
      touchRecent(dir);
    },
    listRecent: async () => [...recent],
    pickFolder: async (title) => (options.pickFolder ?? promptFolder)(title),
    startRun: async (dir, target: RunTarget) => {
      if (!trustStatus(dir).trusted) throw appError('untrusted_graph', 'graph has elevated settings that were not trusted');
      const graph = clone(requireGraph(dir));
      harnesses.assertKnown(graph, knownTargets(graph, target));
      return runner.start(dir, graph, target);
    },
    cancelRun: async (runId) => {
      if (!runner.get(runId)) throw appError('not_found', `run ${runId} not found`);
      runner.cancel(runId);
    },
    cancelNode: async (runId, nodeId) => {
      if (!runner.get(runId)) throw appError('not_found', `run ${runId} not found`);
      runner.cancelNode(runId, nodeId);
    },
    getRun: async (dir, runId) => clone(requireRun(dir, runId).state),
    listRuns: async (dir) =>
      runner
        .list(dir)
        .map(summarise)
        .sort((a, b) => b.run_id.localeCompare(a.run_id)),
    readNodeResult: async (dir, runId, nodeId): Promise<NodeResult> => {
      const run = requireRun(dir, runId);
      if (!(nodeId in run.state.nodes)) throw appError('not_found', `node ${nodeId} not in run ${runId}`);
      return clone(run.results[nodeId] ?? { result_md: null, summary: null, events_tail: [] });
    },
    renderBrief: async (graph, nodeId) => {
      try {
        return renderBrief(graph, nodeId);
      } catch (err) {
        throw appError('not_found', err instanceof Error ? err.message : String(err));
      }
    },
    listMessages: async (dir, runId, nodeId) => clone(requireRun(dir, runId).messages[nodeId] ?? []),
    sendMessage: async (dir, runId, nodeId, text) => {
      requireRun(dir, runId);
      if (text.trim() === '') throw appError('invalid_input', 'message is empty');
      runner.sendMessage(runId, nodeId, text);
    },
    revealGraphDir: async (dir) => {
      requireGraph(dir);
    },
    revealNodeDir: async (dir, nodeId) => {
      if (!requireGraph(dir).nodes.some((n) => n.id === nodeId)) throw appError('not_found', `node ${nodeId} not found`);
    },
    launchOptions: async () => mockLaunchOptions(),
    onRunState: async (handler): Promise<Unlisten> => {
      stateListeners.add(handler);
      return () => stateListeners.delete(handler);
    },
    onNodeEvent: async (handler): Promise<Unlisten> => {
      eventListeners.add(handler);
      return () => eventListeners.delete(handler);
    },
    assistantState: async (dir) => assistant.state(dir),
    assistantSend: async (dir, text) => assistant.send(dir, text),
    assistantCancel: async (dir) => assistant.cancel(dir),
    assistantReset: async (dir) => assistant.reset(dir),
    onAssistantUpdate: async (handler): Promise<Unlisten> => {
      assistantListeners.add(handler);
      return () => assistantListeners.delete(handler);
    },
    listSessions: async (dir, runId) => clone(terminal.list(dir, runId)),
    ptyWrite: async (runId, sid, text) => terminal.write(runId, sid, text),
    ptyResize: async (runId, sid, cols, rows) => terminal.resize(runId, sid, cols, rows),
    ptyReplay: async (runId, sid) => terminal.replay(runId, sid),
    // Not async: window.open must run inside the click's user gesture.
    openSessionWindow: (runId, sid) => {
      try {
        terminal.open(runId, sid);
        return Promise.resolve();
      } catch (err) {
        return Promise.reject(err);
      }
    },
    onPtyData: async (handler): Promise<Unlisten> => {
      ptyListeners.add(handler);
      return () => ptyListeners.delete(handler);
    },
    listPermissionPrompts: async () => clone(permissions.list()),
    answerPermission: async (answer) => permissions.answer(clone(answer)),
    onPermissionPrompt: async (handler): Promise<Unlisten> => {
      promptListeners.add(handler);
      return () => promptListeners.delete(handler);
    },
    onPermissionAnswered: async (handler): Promise<Unlisten> => {
      answeredListeners.add(handler);
      return () => answeredListeners.delete(handler);
    },
    answerPrompt: async (runId, sid, promptId, answer) => {
      prompts.answer(runId, sid, promptId, clone(answer));
      refresh(runId);
    },
    continueNode: async (runId, sid, nodeId) => {
      prompts.continueNode(runId, sid, nodeId);
      refresh(runId);
    },
    listSessionPrompts: async (runId) => clone(prompts.list(runId)),
    onSessionPrompt: async (handler): Promise<Unlisten> => {
      sessionPromptListeners.add(handler);
      return () => sessionPromptListeners.delete(handler);
    },
    onSessionPromptCleared: async (handler): Promise<Unlisten> => {
      clearedListeners.add(handler);
      return () => clearedListeners.delete(handler);
    },
  };
}

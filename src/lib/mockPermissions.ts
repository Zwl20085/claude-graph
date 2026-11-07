/**
 * In-memory stand-in for the backend's approval relay ("Ask me in the app",
 * ASKAPP_CONTRACT). A mock node asks through `ask`; the prompt waits in a
 * queue until `answer` (the dialog) or `cancel` (run / node cancel) resolves
 * it. Remembered scopes auto-answer later identical asks without a prompt but
 * still emit the node events, like the backend:
 * - node scope: per (run, node), until the run ends (in memory);
 * - graph scope: per graph folder (the backend persists it in approvals.json).
 */
import type { AppError, NodeEventKind, PermissionAnswer, PermissionPrompt, PermissionScope } from '../types/contract';

/** Tools whose matching key is their summary (exact command / path / url / query). */
const SUMMARY_KEYED = new Set(['Bash', 'Write', 'Edit', 'WebFetch', 'WebSearch']);
const SUMMARY_FIELD: Readonly<Record<string, string>> = {
  Bash: 'command',
  Write: 'file_path',
  Edit: 'file_path',
  WebSearch: 'query',
  WebFetch: 'url',
};
export const SUMMARY_MAX = 120;
export const CANCELLED_MESSAGE = 'cancelled';

/** Backend-style one-liner of a tool input. */
export function promptSummary(toolName: string, input: unknown): string {
  const field = SUMMARY_FIELD[toolName];
  if (field !== undefined && typeof input === 'object' && input !== null) {
    const v = (input as Record<string, unknown>)[field];
    if (typeof v === 'string') return v;
  }
  const json = JSON.stringify(input) ?? String(input);
  return json.length > SUMMARY_MAX ? json.slice(0, SUMMARY_MAX) : json;
}

/** Matching key of a remembered entry (the backend hashes the canonical JSON; the mock keeps it). */
export function matchKey(toolName: string, input: unknown, summary: string): string {
  return SUMMARY_KEYED.has(toolName) ? summary : JSON.stringify(input) ?? '';
}

/** Text of a `permission_answered` node event. */
export function answeredText(allow: boolean, scope: PermissionScope, note?: string): string {
  return `${allow ? 'allowed' : 'denied'} (${note ?? scope})`;
}

export interface AskRequest {
  graph_dir: string;
  run_id: string;
  node_id: string;
  tool_name: string;
  input: unknown;
}

export interface AskDecision {
  allow: boolean;
  scope: PermissionScope;
  message: string | null;
  /** Answered from a remembered scope, without a prompt. */
  auto: boolean;
}

export interface MockPermissionDeps {
  now: () => Date;
  emitPrompt: (prompt: PermissionPrompt) => void;
  emitAnswered: (id: string) => void;
  emitEvent: (runId: string, nodeId: string, kind: NodeEventKind, text: string) => void;
}

interface Pending {
  prompt: PermissionPrompt;
  graphDir: string;
  key: string;
  onDecision: (d: AskDecision) => void;
}

const appError = (code: string, message: string): AppError => ({ code, message });
const sep = '\u0000';

export function createMockPermissions(deps: MockPermissionDeps) {
  let seq = 0;
  let pending: readonly Pending[] = [];
  /** run\0node\0tool\0key */
  let nodeAllows: ReadonlySet<string> = new Set();
  /** dir\0tool\0key */
  let graphAllows: ReadonlySet<string> = new Set();

  const nodeKey = (runId: string, nodeId: string, tool: string, key: string): string => [runId, nodeId, tool, key].join(sep);
  const graphKey = (dir: string, tool: string, key: string): string => [dir, tool, key].join(sep);

  const resolve = (p: Pending, decision: AskDecision, note?: string): void => {
    pending = pending.filter((x) => x !== p);
    deps.emitAnswered(p.prompt.id);
    deps.emitEvent(p.prompt.run_id, p.prompt.node_id, 'permission_answered', answeredText(decision.allow, decision.scope, note));
    p.onDecision(decision);
  };

  const remembered = (req: AskRequest, key: string): PermissionScope | null => {
    if (nodeAllows.has(nodeKey(req.run_id, req.node_id, req.tool_name, key))) return 'node';
    return graphAllows.has(graphKey(req.graph_dir, req.tool_name, key)) ? 'graph' : null;
  };

  return {
    /** Ask for approval; `onDecision` runs once, when answered, auto-answered or cancelled. */
    ask(req: AskRequest, onDecision: (d: AskDecision) => void): void {
      const summary = promptSummary(req.tool_name, req.input);
      const key = matchKey(req.tool_name, req.input, summary);
      deps.emitEvent(req.run_id, req.node_id, 'permission_prompt', `${req.tool_name}: ${summary}`);
      const scope = remembered(req, key);
      if (scope !== null) {
        deps.emitEvent(req.run_id, req.node_id, 'permission_answered', answeredText(true, scope, `${scope}, remembered`));
        onDecision({ allow: true, scope, message: null, auto: true });
        return;
      }
      seq += 1;
      const prompt: PermissionPrompt = {
        id: `mock-ask-${seq}`,
        run_id: req.run_id,
        node_id: req.node_id,
        tool_name: req.tool_name,
        input: req.input,
        summary,
        suggestions: [],
        ts: deps.now().toISOString(),
      };
      pending = [...pending, { prompt, graphDir: req.graph_dir, key, onDecision }];
      deps.emitPrompt(prompt);
    },
    list(): PermissionPrompt[] {
      return pending.map((p) => p.prompt);
    },
    answer(answer: PermissionAnswer): void {
      const p = pending.find((x) => x.prompt.id === answer.id);
      if (!p) throw appError('not_found', `permission prompt ${answer.id} is not pending`);
      const { run_id, node_id, tool_name } = p.prompt;
      if (answer.allow && answer.scope === 'node') nodeAllows = new Set(nodeAllows).add(nodeKey(run_id, node_id, tool_name, p.key));
      if (answer.allow && answer.scope === 'graph') graphAllows = new Set(graphAllows).add(graphKey(p.graphDir, tool_name, p.key));
      const message = answer.allow ? null : (answer.message?.trim() || 'denied by the user');
      resolve(p, { allow: answer.allow, scope: answer.scope, message, auto: false });
    },
    /** Deny the pending prompts of a run (or of one node of it) with "cancelled". */
    cancel(runId: string, nodeId?: string): void {
      pending
        .filter((p) => p.prompt.run_id === runId && (nodeId === undefined || p.prompt.node_id === nodeId))
        .forEach((p) => resolve(p, { allow: false, scope: 'once', message: CANCELLED_MESSAGE, auto: false }, CANCELLED_MESSAGE));
    },
    /** A run ended: forget its node-scope allows. */
    endRun(runId: string): void {
      nodeAllows = new Set([...nodeAllows].filter((k) => !k.startsWith(`${runId}${sep}`)));
    },
  };
}

export type MockPermissions = ReturnType<typeof createMockPermissions>;

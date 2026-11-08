/**
 * Runtime validation of untrusted graph.json content.
 *
 * Mirrors the Rust side: optional node and settings fields that are missing
 * get the same defaults as `#[serde(default)]` in contract.rs, so a graph that
 * the backend loads is also accepted here. Returns a new normalised object; the
 * input is never mutated.
 */
import {
  DEFAULT_HARNESS,
  GRAPH_VERSION,
  MODELS,
  PERMISSION_MODES,
  RUN_MODES,
  type Deliverable,
  type Edge,
  type Graph,
  type HarnessProfile,
  type Model,
  type Node,
  type PermissionMode,
  type Position,
  type RunMode,
  type Settings,
} from '../types/contract';
import { findCycle } from './graph';

type Obj = Record<string, unknown>;

class ValidationError extends Error {}

const fail = (message: string): never => {
  throw new ValidationError(message);
};

const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v);

function obj(v: unknown, where: string): Obj {
  return isObj(v) ? v : fail(`${where} must be an object`);
}

function str(o: Obj, key: string, where: string, fallback?: string): string {
  const v = o[key];
  if (v === undefined && fallback !== undefined) return fallback;
  return typeof v === 'string' ? v : fail(`${where}.${key} must be a string`);
}

function optStr(o: Obj, key: string, where: string): string | null {
  const v = o[key];
  if (v === undefined || v === null) return null;
  return typeof v === 'string' ? v : fail(`${where}.${key} must be a string or null`);
}

function num(o: Obj, key: string, where: string, fallback: number): number {
  const v = o[key];
  if (v === undefined) return fallback;
  return typeof v === 'number' && Number.isFinite(v) ? v : fail(`${where}.${key} must be a number`);
}

function optUint(o: Obj, key: string, where: string): number | null {
  const v = o[key];
  if (v === undefined || v === null) return null;
  return Number.isInteger(v) && (v as number) >= 0
    ? (v as number)
    : fail(`${where}.${key} must be a non-negative integer or null`);
}

function strArray(v: unknown, where: string): string[] {
  if (!Array.isArray(v) || !v.every((s) => typeof s === 'string')) {
    return fail(`${where} must be an array of strings`);
  }
  return [...v];
}

function oneOf<T extends string>(allowed: readonly T[], v: unknown, where: string, fallback: T): T {
  if (v === undefined) return fallback;
  return (allowed as readonly unknown[]).includes(v)
    ? (v as T)
    : fail(`${where} must be one of ${allowed.join(', ')}`);
}

const DEFAULT_SETTINGS: Settings = {
  max_parallel: 2,
  resources: { cpu: 1, memory: 1, gpu: 1 },
  stop_on_failure: false,
  default_model: 'sonnet',
  default_permission_mode: 'acceptEdits',
  claude_bin: null,
  harness: DEFAULT_HARNESS,
};

function parseSettings(v: unknown): Settings {
  if (v === undefined) return { ...DEFAULT_SETTINGS, resources: { ...DEFAULT_SETTINGS.resources } };
  const o = obj(v, 'settings');
  const resources = o.resources === undefined ? { ...DEFAULT_SETTINGS.resources } : obj(o.resources, 'settings.resources');
  const capacities = Object.fromEntries(
    Object.entries(resources).map(([tag, cap]) =>
      Number.isInteger(cap) && (cap as number) >= 0
        ? [tag, cap as number]
        : fail(`settings.resources.${tag} must be a non-negative integer`),
    ),
  );
  const stop = o.stop_on_failure ?? DEFAULT_SETTINGS.stop_on_failure;
  // run_mode is kept only when present: a missing field means terminal (runModeOf).
  const runMode =
    o.run_mode === undefined ? {} : { run_mode: oneOf<RunMode>(RUN_MODES, o.run_mode, 'settings.run_mode', 'terminal') };
  return {
    ...runMode,
    max_parallel: num(o, 'max_parallel', 'settings', DEFAULT_SETTINGS.max_parallel),
    resources: capacities,
    stop_on_failure: typeof stop === 'boolean' ? stop : fail('settings.stop_on_failure must be a boolean'),
    default_model: oneOf<Model>(MODELS, o.default_model, 'settings.default_model', DEFAULT_SETTINGS.default_model),
    default_permission_mode: oneOf<PermissionMode>(
      PERMISSION_MODES,
      o.default_permission_mode,
      'settings.default_permission_mode',
      DEFAULT_SETTINGS.default_permission_mode,
    ),
    claude_bin: optStr(o, 'claude_bin', 'settings'),
    // Unknown ids load fine; the backend checks them at run start.
    harness: optStr(o, 'harness', 'settings') ?? DEFAULT_HARNESS,
  };
}

function parsePosition(v: unknown, where: string): Position {
  if (v === undefined) return { x: 0, y: 0 };
  const o = obj(v, where);
  return { x: num(o, 'x', where, 0), y: num(o, 'y', where, 0) };
}

function parseDeliverables(v: unknown, where: string): Deliverable[] {
  if (v === undefined) return [];
  if (!Array.isArray(v)) return fail(`${where} must be an array`);
  return v.map((d, i) => {
    const o = obj(d, `${where}[${i}]`);
    return { path: str(o, 'path', `${where}[${i}]`), description: str(o, 'description', `${where}[${i}]`, '') };
  });
}

function parseNode(v: unknown, i: number): Node {
  const where = `nodes[${i}]`;
  const o = obj(v, where);
  const id = str(o, 'id', where);
  if (id.trim() === '') fail(`${where}.id must not be empty`);
  return {
    id,
    title: str(o, 'title', where, ''),
    position: parsePosition(o.position, `${where}.position`),
    prompt: str(o, 'prompt', where, ''),
    goal: str(o, 'goal', where, ''),
    acceptance: str(o, 'acceptance', where, ''),
    scope_in: str(o, 'scope_in', where, ''),
    scope_out: str(o, 'scope_out', where, ''),
    already_known: str(o, 'already_known', where, ''),
    deliverables: parseDeliverables(o.deliverables, `${where}.deliverables`),
    do_nots: o.do_nots === undefined ? [] : strArray(o.do_nots, `${where}.do_nots`),
    model: oneOf<Model>(MODELS, o.model, `${where}.model`, 'inherit'),
    resources: o.resources === undefined ? [] : strArray(o.resources, `${where}.resources`),
    permission_mode: oneOf<PermissionMode>(PERMISSION_MODES, o.permission_mode, `${where}.permission_mode`, 'inherit'),
    allowed_tools:
      o.allowed_tools === undefined || o.allowed_tools === null
        ? null
        : strArray(o.allowed_tools, `${where}.allowed_tools`),
    max_turns: optUint(o, 'max_turns', where),
    working_dir: optStr(o, 'working_dir', where),
    continue_session_of: optStr(o, 'continue_session_of', where),
    extra_context: str(o, 'extra_context', where, ''),
    harness: optStr(o, 'harness', where),
  };
}

function parseEdge(v: unknown, i: number): Edge {
  const where = `edges[${i}]`;
  const o = obj(v, where);
  return { id: str(o, 'id', where), from: str(o, 'from', where), to: str(o, 'to', where) };
}

// ---------------------------------------------------------------------------
// Field rules (mirror of the backend's node validation, FIX_CONTRACT / §6)
// Each returns null when the value is acceptable, else a short user message.
// ---------------------------------------------------------------------------

const NODE_ID_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const RESERVED_STEMS = new Set([
  'con', 'prn', 'aux', 'nul',
  'com1', 'com2', 'com3', 'com4', 'com5', 'com6', 'com7', 'com8', 'com9',
  'lpt1', 'lpt2', 'lpt3', 'lpt4', 'lpt5', 'lpt6', 'lpt7', 'lpt8', 'lpt9',
]);
// eslint-disable-next-line no-control-regex
const TOOL_RE = /^(mcp__[A-Za-z0-9_]+(__[A-Za-z0-9_]+)?|[A-Za-z][A-Za-z0-9_]*)(\([^\x00-\x1f"&|<>^%!]*\))?$/;
const DRIVE_ABS_RE = /^[A-Za-z]:[\\/]/;
const UNC_RE = /^(\\\\|\/\/)/;
const DRIVE_RE = /^[A-Za-z]:/;

/** Node id: folder-safe on every OS, unique case-insensitively among `otherIds`. */
export function nodeIdError(id: string, otherIds: readonly string[] = []): string | null {
  if (!NODE_ID_RE.test(id)) {
    return "Id must start with a letter or digit and use only letters, digits, '.', '_' or '-' (max 128).";
  }
  if (id.endsWith('.')) return 'Id must not end with a dot.';
  const stem = id.split('.')[0]?.toLowerCase() ?? '';
  if (RESERVED_STEMS.has(stem)) return `"${id}" is a reserved name on Windows.`;
  const lower = id.toLowerCase();
  if (otherIds.some((o) => o.toLowerCase() === lower)) return `Id "${id}" is already used (ids are case-insensitive).`;
  return null;
}

/** One --allowedTools entry, e.g. `Read`, `Bash(python:*)`, `mcp__server__tool`. */
export function toolEntryError(entry: string): string | null {
  if (entry.startsWith('-')) return `"${entry}" must not start with "-".`;
  return TOOL_RE.test(entry) ? null : `"${entry}" is not a tool name like Read, Bash(git:*) or mcp__server__tool.`;
}

function segments(path: string): string[] {
  return path.split(/[\\/]+/);
}

/** A relative path that stays inside its base: no `..`, no root, no drive. Empty is an error. */
export function relativePathError(path: string): string | null {
  if (path.trim() === '') return 'Path is empty.';
  if (/^[\\/]/.test(path)) return 'Path must be relative (no leading slash).';
  if (DRIVE_RE.test(path)) return 'Path must be relative (no drive letter).';
  if (segments(path).includes('..')) return 'Path must not contain "..".';
  return null;
}

/** True for `C:\x`, `C:/x` and `/x` (not UNC `\\server` or `//server`). */
export function isAbsolutePath(path: string): boolean {
  return DRIVE_ABS_RE.test(path) || (path.startsWith('/') && !path.startsWith('//'));
}

/** working_dir: null, a safe relative path, or an absolute local path (the latter needs trust). */
export function workingDirError(dir: string | null): string | null {
  if (dir === null) return null;
  if (UNC_RE.test(dir)) return 'Network (UNC) paths are not allowed.';
  if (isAbsolutePath(dir)) return segments(dir).includes('..') ? 'Path must not contain "..".' : null;
  return relativePathError(dir);
}

/** Deliverable path, relative to nodes/<id>/deliverables/. */
export function deliverablePathError(path: string): string | null {
  return relativePathError(path);
}

/** continue_session_of must name a direct upstream node. */
export function continueSessionError(value: string | null, upstream: readonly string[]): string | null {
  if (value === null || upstream.includes(value)) return null;
  return `"${value}" is not a direct upstream node.`;
}

/** Shape of an app-settings claude path (the backend also checks that the file exists). */
export function claudeBinError(path: string): string | null {
  if (UNC_RE.test(path)) return 'Network (UNC) paths are not allowed.';
  if (!isAbsolutePath(path)) return 'Use an absolute path to the claude executable.';
  const file = segments(path).pop() ?? '';
  const dot = file.lastIndexOf('.');
  const stem = (dot > 0 ? file.slice(0, dot) : file).toLowerCase();
  const ext = dot > 0 ? file.slice(dot).toLowerCase() : '';
  if (stem === 'claude' || ((ext === '.exe' || ext === '.cmd') && stem.startsWith('claude'))) return null;
  return 'The file must be claude, claude.exe or claude.cmd.';
}

// ---------------------------------------------------------------------------
// Harness profile rules (mirror of the backend's save_harness validation)
// ---------------------------------------------------------------------------

const HARNESS_ID_RE = /^[a-z0-9][a-z0-9-]{0,31}$/;
const ENV_KEY_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Harness id: lower-case letters, digits and '-', starting with a letter or digit, max 32. */
export function harnessIdError(id: string): string | null {
  return HARNESS_ID_RE.test(id)
    ? null
    : "Id must start with a lower-case letter or digit and use only a-z, 0-9 and '-' (max 32).";
}

/** Environment variable name, e.g. ANTHROPIC_BASE_URL. */
export function envKeyError(key: string): string | null {
  return ENV_KEY_RE.test(key) ? null : `"${key}" is not a variable name (letters, digits and '_', not starting with a digit).`;
}

/** Program path or name; null means the default program of the kind. */
export function harnessCommandError(command: string | null): string | null {
  if (command === null) return null;
  if (command.trim() === '') return 'Command is empty.';
  if (UNC_RE.test(command)) return 'Network (UNC) paths are not allowed.';
  return null;
}

/** Profile fields the backend checks on save_harness. First violation, prefixed with the field name. */
/** Rust-regex syntax the JS engine lacks: inline flags such as `(?i)`, `(?x:…)`. */
const RUST_INLINE_FLAGS = /\(\?[imsxuU]+(?:-[imsxuU]+)?(?=[):])/;

/**
 * A custom option pattern names a `label` group and compiles. The backend
 * (Rust `regex`) is the authority: `(?P<…>)` is read as `(?<…>)`, and a
 * pattern using Rust-only inline flags is left to the backend's check.
 */
export function optionPatternError(pattern: string | null): string | null {
  if (pattern === null || pattern.trim() === '') return null;
  if (!/\(\?P?<label>/.test(pattern)) return 'Name the option text with (?P<label>…).';
  if (RUST_INLINE_FLAGS.test(pattern)) return null;
  try {
    new RegExp(pattern.replace(/\(\?P</g, '(?<'));
    return null;
  } catch {
    return 'Not a valid regular expression.';
  }
}

export function harnessProfileError(profile: HarnessProfile): string | null {
  const custom = profile.kind === 'custom';
  const checks: Array<[string, string | null]> = [
    ['id', harnessIdError(profile.id)],
    ['label', profile.label.trim() === '' ? 'Label is empty.' : null],
    ['command', custom && profile.command === null ? 'A custom harness needs a command.' : harnessCommandError(profile.command)],
    ['env', Object.keys(profile.env).map(envKeyError).find((e) => e !== null) ?? null],
    ['quiet_ms', profile.quiet_ms !== null && !(Number.isInteger(profile.quiet_ms) && profile.quiet_ms > 0) ? 'Must be a positive whole number of ms.' : null],
    ['option_pattern', optionPatternError(profile.option_pattern ?? null)],
    [
      'dsh_profile',
      profile.dsh_profile && !/^[a-z0-9][a-z0-9_-]{0,63}$/.test(profile.dsh_profile) ? 'Use a-z, 0-9, - and _ (max 64).' : null,
    ],
  ];
  const hit = checks.find(([, err]) => err !== null);
  return hit ? `${hit[0]}: ${hit[1]}` : null;
}

/** First rule violation of a node in its graph, prefixed with the field name. */
export function nodeRuleError(graph: Graph, node: Node): string | null {
  const others = graph.nodes.filter((n) => n !== node).map((n) => n.id);
  const upstream = graph.edges.filter((e) => e.to === node.id).map((e) => e.from);
  const checks: Array<[string, string | null]> = [
    ['id', nodeIdError(node.id, others)],
    ['allowed_tools', (node.allowed_tools ?? []).map(toolEntryError).find((e) => e !== null) ?? null],
    ['working_dir', workingDirError(node.working_dir)],
    ['deliverables', node.deliverables.map((d) => deliverablePathError(d.path)).find((e) => e !== null) ?? null],
    ['continue_session_of', continueSessionError(node.continue_session_of, upstream)],
  ];
  const hit = checks.find(([, err]) => err !== null);
  return hit ? `${hit[0]}: ${hit[1]}` : null;
}

function checkStructure(graph: Graph): void {
  const ids = new Set<string>();
  for (const n of graph.nodes) {
    if (ids.has(n.id)) fail(`duplicate node id "${n.id}"`);
    ids.add(n.id);
  }
  const edgeIds = new Set<string>();
  for (const e of graph.edges) {
    if (edgeIds.has(e.id)) fail(`duplicate edge id "${e.id}"`);
    edgeIds.add(e.id);
    if (!ids.has(e.from)) fail(`edge "${e.id}" starts at unknown node "${e.from}"`);
    if (!ids.has(e.to)) fail(`edge "${e.id}" ends at unknown node "${e.to}"`);
  }
  const cycle = findCycle(graph.nodes, graph.edges);
  if (cycle) fail(`graph has a cycle through: ${cycle.join(', ')}`);
  for (const n of graph.nodes) {
    const err = nodeRuleError(graph, n);
    if (err) fail(`node "${n.id}" ${err}`);
  }
}

/** Validate untrusted JSON as a Graph. Returns the normalised Graph or an Error. */
export function validateGraph(json: unknown): Graph | Error {
  try {
    const o = obj(json, 'graph');
    if (o.version !== GRAPH_VERSION) fail(`unsupported graph version ${String(o.version)} (expected ${GRAPH_VERSION})`);
    const nodes = o.nodes === undefined ? [] : Array.isArray(o.nodes) ? o.nodes.map(parseNode) : fail('nodes must be an array');
    const edges = o.edges === undefined ? [] : Array.isArray(o.edges) ? o.edges.map(parseEdge) : fail('edges must be an array');
    const graph: Graph = {
      version: GRAPH_VERSION,
      name: str(o, 'name', 'graph'),
      settings: parseSettings(o.settings),
      nodes,
      edges,
    };
    checkStructure(graph);
    return graph;
  } catch (err) {
    if (err instanceof ValidationError) return new Error(err.message);
    throw err;
  }
}

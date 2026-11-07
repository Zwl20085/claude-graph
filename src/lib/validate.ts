/**
 * Runtime validation of untrusted graph.json content.
 *
 * Mirrors the Rust side: optional node and settings fields that are missing
 * get the same defaults as `#[serde(default)]` in contract.rs, so a graph that
 * the backend loads is also accepted here. Returns a new normalised object; the
 * input is never mutated.
 */
import {
  GRAPH_VERSION,
  MODELS,
  PERMISSION_MODES,
  type Deliverable,
  type Edge,
  type Graph,
  type Model,
  type Node,
  type PermissionMode,
  type Position,
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
  return {
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
  };
}

function parseEdge(v: unknown, i: number): Edge {
  const where = `edges[${i}]`;
  const o = obj(v, where);
  return { id: str(o, 'id', where), from: str(o, 'from', where), to: str(o, 'to', where) };
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

/**
 * Immutable edit operations on a Graph. Every function returns a new Graph (or
 * the same object when nothing changed, so callers can skip marking dirty) and
 * never mutates its input. Referential integrity is kept: removing a node drops
 * its edges, and `continue_session_of` only ever names a direct upstream node.
 */
import type { AppError, Edge, Graph, Node, Position, RunState, RunTarget, Settings } from '../types/contract';
import { findCycle } from './graph';
import { slugify, uniqueSlug } from './slug';
import { nodeIdError, toolEntryError } from './validate';

export type NodePatch = Partial<Omit<Node, 'id'>>;

export function newNode(id: string, title: string, position: Position): Node {
  return {
    id,
    title,
    position: { ...position },
    prompt: '',
    goal: '',
    acceptance: '',
    scope_in: '',
    scope_out: '',
    already_known: '',
    deliverables: [],
    do_nots: [],
    model: 'inherit',
    resources: [],
    permission_mode: 'inherit',
    allowed_tools: null,
    max_turns: null,
    working_dir: null,
    continue_session_of: null,
    extra_context: '',
  };
}

/** Adds a node with a unique slug id derived from `title`. Returns the graph and the id. */
export function addNode(graph: Graph, title: string, position: Position): { graph: Graph; id: string } {
  const id = uniqueSlug(title, graph.nodes.map((n) => n.id.toLowerCase()));
  return { graph: { ...graph, nodes: [...graph.nodes, newNode(id, title, position)] }, id };
}

export function updateNode(graph: Graph, id: string, patch: NodePatch): Graph {
  if (!graph.nodes.some((n) => n.id === id)) return graph;
  return { ...graph, nodes: graph.nodes.map((n) => (n.id === id ? { ...n, ...patch, id } : n)) };
}

export function updateSettings(graph: Graph, patch: Partial<Settings>): Graph {
  return { ...graph, settings: { ...graph.settings, ...patch } };
}

/** Ids of direct upstream nodes of `id`, in edge order. */
/** A node with neither a prompt nor a goal has nothing to do and must not run. */
export function isEmptyNode(node: Pick<Node, 'prompt' | 'goal'>): boolean {
  return node.prompt.trim() === '' && node.goal.trim() === '';
}

export function upstreamOf(graph: Graph, id: string): string[] {
  return graph.edges.filter((e) => e.to === id).map((e) => e.from);
}

/** Positions for nodes that exist; unchanged positions keep the same graph object. */
export function applyPositions(graph: Graph, positions: Readonly<Record<string, Position>>): Graph {
  let changed = false;
  const nodes = graph.nodes.map((n) => {
    const p = positions[n.id];
    if (!p || (p.x === n.position.x && p.y === n.position.y)) return n;
    changed = true;
    return { ...n, position: { x: p.x, y: p.y } };
  });
  return changed ? { ...graph, nodes } : graph;
}

/** Clears continue_session_of wherever it no longer names a direct upstream node. */
function repairSessionLinks(graph: Graph): Graph {
  let changed = false;
  const nodes = graph.nodes.map((n) => {
    if (n.continue_session_of === null) return n;
    const ok = graph.edges.some((e) => e.from === n.continue_session_of && e.to === n.id);
    if (ok) return n;
    changed = true;
    return { ...n, continue_session_of: null };
  });
  return changed ? { ...graph, nodes } : graph;
}

/** Removes nodes (and every edge touching them) and edges by id. */
export function removeElements(graph: Graph, nodeIds: readonly string[], edgeIds: readonly string[]): Graph {
  const dropNodes = new Set(nodeIds);
  const dropEdges = new Set(edgeIds);
  const nodes = graph.nodes.filter((n) => !dropNodes.has(n.id));
  const edges = graph.edges.filter(
    (e) => !dropEdges.has(e.id) && !dropNodes.has(e.from) && !dropNodes.has(e.to),
  );
  if (nodes.length === graph.nodes.length && edges.length === graph.edges.length) return graph;
  return repairSessionLinks({ ...graph, nodes, edges });
}

export function nextEdgeId(edges: readonly Edge[]): string {
  const taken = new Set(edges.map((e) => e.id));
  const max = edges.reduce((m, e) => {
    const match = /^e(\d+)$/.exec(e.id);
    return match ? Math.max(m, Number(match[1])) : m;
  }, 0);
  let n = max + 1;
  while (taken.has(`e${n}`)) n += 1;
  return `e${n}`;
}

export type ConnectResult = { ok: true; graph: Graph; edgeId: string } | { ok: false; error: string };

/** Adds an edge from -> to, refusing self loops, unknown ends, duplicates and cycles. */
export function connect(graph: Graph, from: string, to: string): ConnectResult {
  const ids = new Set(graph.nodes.map((n) => n.id));
  if (!ids.has(from) || !ids.has(to)) return { ok: false, error: 'Both ends must be existing nodes.' };
  if (from === to) return { ok: false, error: 'A node cannot depend on itself.' };
  if (graph.edges.some((e) => e.from === from && e.to === to)) {
    return { ok: false, error: `Edge ${from} → ${to} already exists.` };
  }
  const edgeId = nextEdgeId(graph.edges);
  const edges = [...graph.edges, { id: edgeId, from, to }];
  const cycle = findCycle(graph.nodes, edges);
  if (cycle) return { ok: false, error: `Refused: ${from} → ${to} would create a cycle.` };
  return { ok: true, graph: { ...graph, edges }, edgeId };
}

export type RenameResult = { ok: true; graph: Graph } | { ok: false; error: string };

/** Renames a node id and every reference to it (edges, continue_session_of). */
export function renameNode(graph: Graph, oldId: string, newId: string): RenameResult {
  if (oldId === newId) return { ok: true, graph };
  if (!graph.nodes.some((n) => n.id === oldId)) return { ok: false, error: `Unknown node "${oldId}".` };
  const error = nodeIdError(
    newId,
    graph.nodes.filter((n) => n.id !== oldId).map((n) => n.id),
  );
  if (error) return { ok: false, error };
  const swap = (id: string): string => (id === oldId ? newId : id);
  return {
    ok: true,
    graph: {
      ...graph,
      nodes: graph.nodes.map((n) => ({
        ...n,
        id: swap(n.id),
        continue_session_of: n.continue_session_of === null ? null : swap(n.continue_session_of),
      })),
      edges: graph.edges.map((e) => ({ ...e, from: swap(e.from), to: swap(e.to) })),
    },
  };
}

const NEW_NODE_ID = /^new-node(-\d+)?$/;

/**
 * Whether `id` still follows `title`: it is `slug(title)` (with a uniqueness
 * suffix -2, -3, … that `uniqueSlug` may have added) or a `new-node` id.
 * A hand-typed id that differs from the slug never follows again.
 */
export function followsTitle(id: string, title: string): boolean {
  if (NEW_NODE_ID.test(id)) return true;
  const base = slugify(title);
  return id === base || new RegExp(`^${base}-\\d+$`).test(id);
}

/**
 * Sets a node's title; when `mayRename` (the node has never run) and its id
 * still follows the old title, the id is re-derived from the new title, made
 * unique against the other ids (case-insensitive), and every reference
 * (edges, continue_session_of) is rewritten. Returns the graph and the
 * node's (possibly new) id. An id the rename rules refuse is kept.
 */
export function retitleNode(graph: Graph, id: string, title: string, mayRename: boolean): { graph: Graph; id: string } {
  const node = graph.nodes.find((n) => n.id === id);
  if (!node) return { graph, id };
  const titled = updateNode(graph, id, { title });
  if (!mayRename || !followsTitle(node.id, node.title)) return { graph: titled, id };
  const nextId = uniqueSlug(title, graph.nodes.filter((n) => n.id !== id).map((n) => n.id.toLowerCase()));
  if (nextId === id) return { graph: titled, id };
  const renamed = renameNode(titled, id, nextId);
  return renamed.ok ? { graph: renamed.graph, id: nextId } : { graph: titled, id };
}

/** Empty graph as `create_graph` would write it (used by the mock). */
export function emptyGraph(name: string): Graph {
  return {
    version: 1,
    name,
    settings: {
      max_parallel: 2,
      resources: { cpu: 1, memory: 1, gpu: 1 },
      stop_on_failure: false,
      default_model: 'sonnet',
      default_permission_mode: 'claude_settings',
      claude_bin: null,
    },
    nodes: [],
    edges: [],
  };
}

/** Ids a target selects: all, `from` + transitive dependents, or failed/skipped/cancelled of a run + dependents. */
export function targetSet(graph: Graph, target: RunTarget, previous: (runId: string) => RunState | undefined): string[] {
  const descendants = (seeds: string[]): Set<string> => {
    const out = new Set(seeds);
    const queue = [...seeds];
    while (queue.length > 0) {
      const id = queue.shift() as string;
      for (const e of graph.edges) {
        if (e.from === id && !out.has(e.to)) {
          out.add(e.to);
          queue.push(e.to);
        }
      }
    }
    return out;
  };
  const ids = graph.nodes.map((n) => n.id);
  if (target === 'all') return ids;
  if ('from' in target) {
    if (!ids.includes(target.from)) throw { code: 'not_found', message: `node ${target.from} not found` } as AppError;
    const set = descendants([target.from]);
    return ids.filter((id) => set.has(id));
  }
  const prev = previous(target.failed_of);
  if (!prev) throw { code: 'not_found', message: `run ${target.failed_of} not found` } as AppError;
  // Same rule as the backend scheduler: every node of that run that did not
  // succeed (failed, blocked, skipped, cancelled) is rerun, with its dependents.
  const seeds = Object.entries(prev.nodes)
    .filter(([id, n]) => n.status !== 'succeeded' && ids.includes(id))
    .map(([id]) => id);
  if (seeds.length === 0) {
    throw { code: 'not_found', message: `run ${target.failed_of} has no failed, blocked, skipped or cancelled nodes` } as AppError;
  }
  const set = descendants(seeds);
  return ids.filter((id) => set.has(id));
}

/** Ids of empty nodes (no prompt, no goal) among `ids`. */
export function emptyNodeIds(graph: Graph, ids: readonly string[]): string[] {
  const wanted = new Set(ids);
  return graph.nodes.filter((n) => wanted.has(n.id) && isEmptyNode(n)).map((n) => n.id);
}

/** Split on commas outside parentheses: `Bash(git add, git commit), Read` is two entries. */
export function splitTopLevel(text: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let current = '';
  for (const ch of text) {
    if (ch === '(') depth += 1;
    else if (ch === ')') depth = Math.max(0, depth - 1);
    if (ch === ',' && depth === 0) {
      parts.push(current);
      current = '';
    } else {
      current += ch;
    }
  }
  parts.push(current);
  return parts.map((s) => s.trim()).filter((s) => s !== '');
}

/** Comma text -> array; empty -> null (= no --allowedTools flag). Commas inside (...) do not split. */
export function parseToolList(text: string): string[] | null {
  const items = splitTopLevel(text);
  return items.length === 0 ? null : items;
}

/**
 * Tools text as typed: the valid entries (what is committed; null = none) and
 * the first error message (shown inline). Invalid entries are never committed.
 */
export function readToolList(text: string): { valid: string[] | null; error: string | null } {
  const entries = splitTopLevel(text);
  const errors = entries.map(toolEntryError).filter((e): e is string => e !== null);
  const valid = entries.filter((e) => toolEntryError(e) === null);
  const more = errors.length > 1 ? ` (+${errors.length - 1} more)` : '';
  const error = errors.length === 0 ? null : `${errors[0]}${more} Invalid entries are not saved.`;
  return { valid: valid.length === 0 ? null : valid, error };
}

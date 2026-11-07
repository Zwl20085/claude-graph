/** Pure graph helpers. */
import type { Edge, Node } from '../types/contract';

/**
 * Kahn's algorithm. Returns null if the graph is acyclic, otherwise the ids of
 * the nodes that could not be ordered (every node on a cycle, plus nodes
 * downstream of one), sorted ascending. Edges whose endpoints are not in
 * `nodes` are ignored.
 */
export function findCycle(
  nodes: ReadonlyArray<Pick<Node, 'id'>>,
  edges: ReadonlyArray<Pick<Edge, 'from' | 'to'>>,
): string[] | null {
  const ids = new Set(nodes.map((n) => n.id));
  const inDegree = new Map<string, number>([...ids].map((id) => [id, 0]));
  const outgoing = new Map<string, string[]>([...ids].map((id) => [id, []]));
  for (const { from, to } of edges) {
    if (!ids.has(from) || !ids.has(to)) continue;
    outgoing.get(from)?.push(to);
    inDegree.set(to, (inDegree.get(to) ?? 0) + 1);
  }

  const queue = [...ids].filter((id) => inDegree.get(id) === 0);
  let visited = 0;
  while (queue.length > 0) {
    const id = queue.shift() as string;
    visited += 1;
    for (const next of outgoing.get(id) ?? []) {
      const d = (inDegree.get(next) ?? 0) - 1;
      inDegree.set(next, d);
      if (d === 0) queue.push(next);
    }
  }

  if (visited === ids.size) return null;
  return [...inDegree.entries()]
    .filter(([, d]) => d > 0)
    .map(([id]) => id)
    .sort();
}

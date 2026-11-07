/**
 * Trust items: the elevated settings of a graph that need the user's consent
 * before a run (FIX_CONTRACT `graph_trust_status`). The backend is the
 * authority; this module renders its items as text and gives the mock the
 * same rules.
 */
import type { Graph, PermissionMode, TrustItem } from '../types/contract';
import { isAbsolutePath } from './validate';

const ELEVATED_MODES: Partial<Record<PermissionMode, TrustItem['kind']>> = {
  bypassPermissions: 'bypass_permissions',
  dontAsk: 'dont_ask',
};

const isBashTool = (entry: string): boolean => entry === 'Bash' || entry.startsWith('Bash(');

/** Trust items of a graph, graph-level first, then per node in node order. */
export function trustItems(graph: Graph): TrustItem[] {
  const items: TrustItem[] = [];
  const defaultKind = ELEVATED_MODES[graph.settings.default_permission_mode];
  if (defaultKind) items.push({ node_id: null, kind: defaultKind, detail: graph.settings.default_permission_mode });
  if (graph.settings.claude_bin !== null) {
    items.push({ node_id: null, kind: 'claude_bin_in_graph', detail: graph.settings.claude_bin });
  }
  for (const node of graph.nodes) {
    const modeKind = ELEVATED_MODES[node.permission_mode];
    if (modeKind) items.push({ node_id: node.id, kind: modeKind, detail: node.permission_mode });
    const bash = (node.allowed_tools ?? []).filter(isBashTool);
    if (bash.length > 0) items.push({ node_id: node.id, kind: 'bash_tool', detail: bash.join(', ') });
    if (node.working_dir !== null && isAbsolutePath(node.working_dir)) {
      items.push({ node_id: node.id, kind: 'absolute_working_dir', detail: node.working_dir });
    }
  }
  return items;
}

/** Stable fingerprint of the elevated fields (the mock's stand-in for the backend hash). */
export function trustFingerprint(items: readonly TrustItem[]): string {
  return JSON.stringify(items.map((i) => [i.node_id, i.kind, i.detail]));
}

/** Human text of one item for the trust dialog. */
export function trustItemText(item: TrustItem): string {
  const node = item.node_id === null ? null : `Node ${item.node_id}`;
  switch (item.kind) {
    case 'bypass_permissions':
      return node
        ? `${node} runs with bypassPermissions (Claude may run any command)`
        : 'Default permission mode is bypassPermissions (Claude may run any command)';
    case 'dont_ask':
      return node
        ? `${node} runs with dontAsk (Claude runs its allowed tools without asking)`
        : 'Default permission mode is dontAsk (Claude runs its allowed tools without asking)';
    case 'bash_tool':
      return `${node ?? 'A node'} allows Bash tools${item.detail ? `: ${item.detail}` : ''}`;
    case 'absolute_working_dir':
      return `${node ?? 'A node'} uses an absolute working directory: ${item.detail}`;
    case 'claude_bin_in_graph':
      return 'graph.json names a claude binary (ignored)';
    default:
      return `${node ? `${node}: ` : ''}${String(item.kind)} ${item.detail}`.trim();
  }
}

#!/usr/bin/env node
// Probe for Claude Code's --permission-prompt-tool wire format.
// A minimal MCP stdio server with one tool `approve`. Every JSON-RPC line it receives is
// appended to the file named by env PROBE_LOG. tools/call answers allow (echoing the
// input as updatedInput) unless env PROBE_DENY is set, then it answers deny.
// Usage: see docs/CLAUDE_CLI_NOTES.md ("Permission prompt tool").
import { appendFileSync } from 'node:fs';
import { createInterface } from 'node:readline';

const LOG = process.env.PROBE_LOG;
const DENY = Boolean(process.env.PROBE_DENY);

function log(line) {
  if (LOG) appendFileSync(LOG, line + '\n');
}

function send(msg) {
  const text = JSON.stringify(msg);
  log('<< ' + text);
  process.stdout.write(text + '\n');
}

function decisionFor(args) {
  if (DENY) return { behavior: 'deny', message: 'probe says no' };
  return { behavior: 'allow', updatedInput: (args && args.input) || {} };
}

function handle(req) {
  const { id, method, params } = req;
  if (method === 'initialize') {
    send({
      jsonrpc: '2.0',
      id,
      result: {
        protocolVersion: (params && params.protocolVersion) || '2025-06-18',
        capabilities: { tools: {} },
        serverInfo: { name: 'probe', version: '0.0.1' },
      },
    });
  } else if (method === 'ping') {
    send({ jsonrpc: '2.0', id, result: {} });
  } else if (method === 'tools/list') {
    send({
      jsonrpc: '2.0',
      id,
      result: {
        tools: [
          {
            name: 'approve',
            description: 'Approve or deny a tool call',
            inputSchema: { type: 'object', additionalProperties: true },
          },
        ],
      },
    });
  } else if (method === 'tools/call') {
    const decision = decisionFor(params && params.arguments);
    send({
      jsonrpc: '2.0',
      id,
      result: { content: [{ type: 'text', text: JSON.stringify(decision) }] },
    });
  } else if (id !== undefined) {
    send({ jsonrpc: '2.0', id, error: { code: -32601, message: 'method not found' } });
  }
}

const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => {
  log('>> ' + line);
  try {
    handle(JSON.parse(line));
  } catch (e) {
    log('!! ' + String(e));
  }
});

// FAKE_ASK support for fake-claude.mjs (docs/ARCHITECTURE.md "Ask me in the app").
//
// With `--mcp-config <inline JSON>` and `--permission-prompt-tool mcp__<server>__<tool>`
// the fake starts the named stdio MCP server with its env, runs the handshake the real
// CLI runs (initialize, notifications/initialized, tools/list), and reports the server
// in its init event. A prompt marker `FAKE_ASK=<tool>` then calls the prompt tool with
// `{tool_name:<tool>, input:{command:"echo hi"}, tool_use_id:"toolu_fake"}` (the format
// recorded in docs/CLAUDE_CLI_NOTES.md), `FAKE_ASK_COUNT=<n>` times (default 1). Each
// answer appends one line to `asked-allowed.txt` (allow) or `asked-denied.txt` (deny,
// with the message) in the cwd. `FAKE_MCP_FAIL` reports the server as failed instead.
import { spawn } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { join } from 'node:path';

const argValue = (name) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
};

/** `{ server, tool }` from `--permission-prompt-tool mcp__<server>__<tool>`, else null. */
function promptTool() {
  const name = argValue('--permission-prompt-tool');
  const m = name && name.match(/^mcp__(.+)__([^_].*)$/);
  return m ? { server: m[1], tool: m[2] } : null;
}

/** A JSON-RPC client over a child's stdio. */
function client(child) {
  const waiting = new Map();
  let next = 0;
  createInterface({ input: child.stdout }).on('line', (line) => {
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    const done = waiting.get(msg.id);
    if (done) {
      waiting.delete(msg.id);
      done(msg);
    }
  });
  const request = (method, params) =>
    new Promise((res) => {
      const id = next++;
      waiting.set(id, res);
      child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
    });
  const notify = (method) => child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method })}\n`);
  return { request, notify };
}

/**
 * Start the prompt tool's server and handshake. Returns
 * `{ mcpServers, ask(prompt), close() }`; `mcpServers` goes into the init event.
 */
export async function startAsk(prompt) {
  const pt = promptTool();
  const raw = argValue('--mcp-config');
  if (!pt || !raw) return { mcpServers: undefined, ask: async () => {}, close() {} };
  const failed = (error) => ({
    mcpServers: [{ name: pt.server, status: 'failed' }],
    mcpServerErrors: error,
    ask: async () => {},
    close() {},
  });
  if (prompt.includes('FAKE_MCP_FAIL')) return failed('FAKE_MCP_FAIL was present in the prompt');
  const entry = JSON.parse(raw).mcpServers?.[pt.server];
  if (!entry) return failed(`no server ${pt.server} in --mcp-config`);
  const child = spawn(entry.command, entry.args ?? [], {
    env: { ...process.env, ...(entry.env ?? {}) },
    stdio: ['pipe', 'pipe', 'inherit'],
    windowsHide: true,
  });
  const rpc = client(child);
  const init = await rpc.request('initialize', {
    protocolVersion: '2025-11-25',
    capabilities: {},
    clientInfo: { name: 'fake-claude', version: '0' },
  });
  if (!init.result) return failed(JSON.stringify(init.error ?? 'initialize failed'));
  rpc.notify('notifications/initialized');
  const list = await rpc.request('tools/list', {});
  if (!list.result?.tools?.some((t) => t.name === pt.tool)) return failed(`no tool ${pt.tool}`);
  const ask = async () => {
    const tool = prompt.match(/FAKE_ASK=(\S+)/)?.[1];
    if (!tool) return;
    const count = Number(prompt.match(/FAKE_ASK_COUNT=(\d+)/)?.[1] ?? 1) || 1;
    for (let i = 0; i < count; i++) {
      const reply = await rpc.request('tools/call', {
        name: pt.tool,
        arguments: { tool_name: tool, input: { command: 'echo hi' }, tool_use_id: 'toolu_fake' },
        _meta: { 'claudecode/toolUseId': 'toolu_fake', progressToken: i },
      });
      const text = reply.result?.content?.[0]?.text ?? '{}';
      const decision = JSON.parse(text);
      if (decision.behavior === 'allow') {
        appendFileSync(join(process.cwd(), 'asked-allowed.txt'), `${JSON.stringify(decision.updatedInput)}\n`);
      } else {
        appendFileSync(join(process.cwd(), 'asked-denied.txt'), `${decision.message ?? ''}\n`);
      }
    }
  };
  return {
    mcpServers: [{ name: pt.server, status: 'connected' }],
    ask,
    close: () => child.kill(),
  };
}

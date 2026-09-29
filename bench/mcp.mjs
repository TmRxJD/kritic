// Minimal stdio MCP client for driving the kritic server from scripts.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

export async function connect(exe, env = {}) {
  const child = spawn(exe, [], { stdio: ["pipe", "pipe", "ignore"], windowsHide: true, env: { ...process.env, ...env } });
  const pending = new Map();
  let nextId = 1;
  createInterface({ input: child.stdout }).on("line", (line) => {
    let m;
    try { m = JSON.parse(line); } catch { return; }
    const w = pending.get(m.id);
    if (!w) return;
    pending.delete(m.id);
    m.error ? w.reject(new Error(JSON.stringify(m.error))) : w.resolve(m.result);
  });
  const request = (method, params) => new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  });
  await request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "kritic-bench", version: "1" } });
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
  return {
    /** Calls a tool; returns { result, ms, chars } where chars is what the agent's context receives. */
    async call(name, args) {
      const t0 = Date.now();
      const r = await request("tools/call", { name, arguments: args });
      const text = (r.content ?? []).map((c) => c.text ?? "").join("");
      return { result: r.structuredContent ?? safeJson(text), ms: Date.now() - t0, chars: text.length, isError: !!r.isError, text };
    },
    close: () => child.kill(),
  };
}

const safeJson = (t) => { try { return JSON.parse(t); } catch { return { error: t }; } };

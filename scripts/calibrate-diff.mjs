// Calibrate critic_score's diff mode with paired hunks: each clean hunk comes
// from this repo's own history (tests passed when it was committed); its buggy
// twin is the same hunk with one mutation in an added line. A useful rule scores
// the twin higher than the original; a threshold is only worth what it separates.
//
//   node scripts/calibrate-diff.mjs [maxPairs=30] [exe]   (exe defaults to $KRITIC_EXE, then target/release/kritic)
// Writes logs/calibration-diff.json and prints per-rule separation.
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";

const maxPairs = Number(process.argv[2] ?? 30);
const root = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
const exe = process.argv[3] ?? process.env.KRITIC_EXE ?? `${root}/target/release/kritic${process.platform === "win32" ? ".exe" : ""}`;

const MUTATIONS = [
  [/ < /, " > "], [/ > /, " < "], [/ <= /, " > "], [/ >= /, " < "],
  [/ == /, " != "], [/ != /, " == "], [/ && /, " || "], [/ \|\| /, " && "],
  [/ \+ 1\b/, " - 1"], [/ - 1\b/, " + 1"], [/\btrue\b/, "false"], [/\bfalse\b/, "true"],
];

function hunksFromHistory() {
  const log = execFileSync("git", ["log", "-p", "--no-color", "-U3", "--format=commit %h", "--", "src/*.rs"], { cwd: root, encoding: "utf8", maxBuffer: 1 << 28 });
  const out = [];
  let file = "", commit = "", header = "", body = [];
  const flush = () => {
    if (header && body.length) out.push({ commit, file, text: `diff --git a/${file} b/${file}\n${header}\n${body.join("\n")}\n` });
    header = ""; body = [];
  };
  for (const line of log.split("\n")) {
    if (line.startsWith("commit ")) { flush(); commit = line.slice(7); }
    else if (line.startsWith("diff --git")) { flush(); file = line.split(" b/")[1] ?? ""; }
    else if (line.startsWith("@@")) { flush(); header = line; }
    else if (header && /^[ +-]/.test(line) && !line.startsWith("+++") && !line.startsWith("---")) body.push(line);
  }
  flush();
  return out.filter((h) => h.text.length < 1200 && h.text.split("\n").some((l) => l.startsWith("+") && !/^\+\s*(\/\/|$)/.test(l)));
}

function mutate(text) {
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    if (!l.startsWith("+") || /^\+\s*\/\//.test(l)) continue;
    for (const [from, to] of MUTATIONS) {
      if (from.test(l)) {
        const copy = [...lines];
        copy[i] = l.replace(from, to);
        return { text: copy.join("\n"), mutation: `${from.source} -> ${to.trim()} at +line ${i}` };
      }
    }
  }
  return undefined;
}

// ---- minimal stdio MCP client
const child = spawn(exe, [], { stdio: ["pipe", "pipe", "ignore"], windowsHide: true });
const pending = new Map();
let nextId = 1;
createInterface({ input: child.stdout }).on("line", (line) => {
  let m; try { m = JSON.parse(line); } catch { return; }
  const w = pending.get(m.id); if (!w) return; pending.delete(m.id);
  m.error ? w.reject(new Error(JSON.stringify(m.error))) : w.resolve(m.result);
});
const request = (method, params) => new Promise((resolve, reject) => {
  const id = nextId++; pending.set(id, { resolve, reject });
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
});
await request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "calibrate-diff", version: "1" } });
child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
const score = async (diff) => (await request("tools/call", { name: "critic_score", arguments: { diff, task: "(judge the change on its own)" } })).structuredContent;

const pairs = [];
for (const h of hunksFromHistory()) {
  if (pairs.length >= maxPairs) break;
  const m = mutate(h.text);
  if (!m) continue;
  const clean = await score(h.text);
  const buggy = await score(m.text);
  if (!clean?.kev_ok || !buggy?.kev_ok) continue;
  const pick = (r) => ({ approval: r.approval, p_severe: r.p_severe_max, p_bug: 1 - r.p_correct_min, reasons: r.reasons });
  pairs.push({ commit: h.commit, file: h.file, mutation: m.mutation, clean: pick(clean), buggy: pick(buggy) });
  process.stderr.write(`${pairs.length} ${h.file} sev ${clean.p_severe_max}->${buggy.p_severe_max} bug ${(1 - clean.p_correct_min).toFixed(2)}->${(1 - buggy.p_correct_min).toFixed(2)}\n`);
}
child.kill();

const summarize = (key) => {
  const higher = pairs.filter((p) => p.buggy[key] > p.clean[key]).length;
  const at = (t) => ({ threshold: t, flagClean: pairs.filter((p) => p.clean[key] > t).length, flagBuggy: pairs.filter((p) => p.buggy[key] > t).length });
  return { buggyHigher: `${higher}/${pairs.length}`, meanClean: mean(pairs.map((p) => p.clean[key])), meanBuggy: mean(pairs.map((p) => p.buggy[key])), thresholds: [0.3, 0.5, 0.6, 0.7, 0.8].map(at) };
};
const mean = (v) => Math.round((v.reduce((s, x) => s + x, 0) / v.length) * 1000) / 1000;
const report = {
  pairs: pairs.length,
  p_severe: summarize("p_severe"),
  p_bug: summarize("p_bug"),
  approval: { cleanRejected: pairs.filter((p) => p.clean.approval === false).length, buggyRejected: pairs.filter((p) => p.buggy.approval === false).length },
  detail: pairs,
};
mkdirSync(`${root}/logs`, { recursive: true });
writeFileSync(`${root}/logs/calibration-diff.json`, JSON.stringify(report, null, 1));
const { detail, ...headline } = report;
console.log(JSON.stringify(headline, null, 1));

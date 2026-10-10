// kritic benchmark: the same agent changes reviewed without kritic (the project's own tests,
// which an agent already runs) and with kritic (those tests plus the end-of-task review the
// kritic skill prescribes). Every case has a defective variant and a correct one, so both the
// catch rate and the false-rejection rate are measured.
//
//   node bench/run.mjs [--exe <kritic binary>] [--only <case id>] [--out bench/results]
//
// Needs the kritic server with Kev (laya) available; security cases need the scanners and network
// (osv-scanner); UI cases need the Playwright harness; messy-layout needs Ollama + the vision model.
// A tool that cannot run reports approval null; the report counts that as "no verdict", never as
// a catch or a pass.
import { execFileSync, spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { mkdirSync, mkdtempSync, readFileSync, statSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join } from "node:path";
import { connect } from "./mcp.mjs";
import { BENCH_FIXTURE_DIR, CASES, FIXTURE_DIR } from "./cases.mjs";
import { fixturePath } from "./fixture-path.mjs";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : fallback;
};
const root = decodeURIComponent(new URL("..", import.meta.url).pathname).replace(/^\/([A-Za-z]:)/, "$1");
const targetDir = () => JSON.parse(execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], { cwd: root, encoding: "utf8" })).target_directory;
const exe = arg("--exe", process.env.KRITIC_EXE ?? join(targetDir(), "release", process.platform === "win32" ? "kritic.exe" : "kritic"));
const only = arg("--only", null);
const outDir = arg("--out", join(root, "bench", "results"));

// ---- static server for the UI fixtures
const TYPES = { ".html": "text/html", ".css": "text/css", ".png": "image/png" };
const server = createServer((req, res) => {
  // bench/fixtures overrides the harness fixtures (a clean variant that must also work)
  const path = fixturePath(req.url, [BENCH_FIXTURE_DIR, FIXTURE_DIR]);
  if (!path) { res.writeHead(404).end("not found"); return; }
  try {
    const body = readFileSync(path);
    res.writeHead(200, { "content-type": TYPES[extname(path)] ?? "application/octet-stream" }).end(body);
  } catch {
    res.writeHead(404).end("not found");
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const origin = `http://127.0.0.1:${server.address().port}`;

// ---- project setup
function write(dir, files) {
  for (const [rel, text] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, rel)), { recursive: true });
    writeFileSync(join(dir, rel), text);
  }
}
function project(c, variant) {
  const dir = mkdtempSync(join(tmpdir(), `kritic-bench-${c.id}-${variant}-`));
  write(dir, c.base);
  const git = (...a) => execFileSync("git", a, { cwd: dir, stdio: "ignore" });
  git("init", "-q");
  git("-c", "user.name=bench", "-c", "user.email=bench@example.test", "add", "-A");
  git("-c", "user.name=bench", "-c", "user.email=bench@example.test", "commit", "-q", "-m", "base");
  const change = c[variant]();
  write(dir, change);
  return { dir, changed: Object.keys(change).map((f) => join(dir, f)) };
}

function baseline(c, dir) {
  if (!c.test) return { ran: false, rejected: false, ms: 0, note: "no automated check exists for this change" };
  const t0 = Date.now();
  const options = { cwd: dir, shell: false, encoding: "utf8", timeout: 120_000 };
  let r;
  if (c.test === "node --test") r = spawnSync("node", ["--test"], options);
  else if (c.test === "python -m unittest -q") r = spawnSync("python", ["-m", "unittest", "-q"], options);
  else throw new Error(`unsupported benchmark test command: ${c.test}`);
  return { ran: true, rejected: r.status !== 0, ms: Date.now() - t0, exit: r.status };
}

const fullSize = (r) => {
  const p = r?.full_result;
  try { return p ? statSync(p).size : 0; } catch { return 0; }
};

async function review(k, c, variant, dir, changed) {
  const calls = [
    ["critic_score", { git_diff: true, base_dir: dir, task: c.task, revision: 0 }],
    ["artifact_validate", { paths: changed, commands: c.test ? [c.test] : [], base_dir: dir, task: c.task, use_model: false }],
  ];
  for (const t of c.tools) {
    if (t === "security_critic") calls.push([t, { base_dir: dir, triage: true }]);
    if (t === "code_quality_critic") calls.push([t, { base_dir: dir, paths: changed }]);
    if (t === "ui_test") calls.push([t, { url: origin + c.url[variant], max_pages: 2 }]);
    if (t === "visual_critic") calls.push([t, { url: origin + c.url[variant], metadata: c.task }]);
    if (t === "resource_critic") calls.push([t, { command: ["node", "worker.mjs"], cwd: dir, duration_s: 30, workload: "a worker caching responses for simulated requests with unique paths" }]);
  }
  const tools = [];
  for (const [name, args] of calls) {
    let r;
    try {
      r = await k.call(name, args);
    } catch (e) {
      tools.push({ tool: name, approval: null, error: String(e).slice(0, 300), ms: 0, chars: 0 });
      continue;
    }
    const res = r.result ?? {};
    tools.push({
      tool: name,
      approval: r.isError ? null : res.approval ?? null,
      ms: r.ms,
      response_chars: r.chars,
      full_result_bytes: fullSize(res),
      evidence: evidence(name, res),
      ...(r.isError ? { error: r.text.slice(0, 300) } : {}),
    });
  }
  return tools;
}

// The facts behind a verdict, short enough for the report.
function evidence(name, r) {
  const reasons = (r.reasons ?? []).slice(0, 4).map((x) => String(x).slice(0, 200));
  switch (name) {
    case "critic_score": return { kev_ok: r.kev_ok, reasons, flagged: r.flagged_chunks?.length ?? 0 };
    case "security_critic": return { score: r.security_score, counts: r.counts, reasons };
    case "code_quality_critic": return { score: r.quality_score, reasons };
    case "ui_test": return { breakages: (r.breakages ?? []).slice(0, 8).map((x) => String(x).slice(0, 120)), error: r.error };
    case "visual_critic": return { scores: r.scores, min: r.min_score, error: r.error };
    case "resource_critic": return { violations: r.budget_violations, reasons, error: r.error };
    default: return { reasons };
  }
}

const verdict = (tools) => {
  if (tools.some((t) => t.approval === false)) return "rejected";
  if (tools.some((t) => t.approval === null)) return "no-verdict";
  return "approved";
};

// ---- run
const k = await connect(exe);
const status = (await k.call("status", { start: true })).result;
if (!status?.laya_up) console.error(`warning: laya not up (${status?.start_error ?? "unknown"}); critic_score will report no verdict`);
const results = [];
for (const c of CASES.filter((c) => !only || c.id === only)) {
  for (const variant of ["bad", "good"]) {
    const { dir, changed } = project(c, variant);
    const base = baseline(c, dir);
    const tools = await review(k, c, variant, dir, changed);
    const kv = verdict(tools);
    const row = {
      case: c.id, category: c.category, variant, defect: variant === "bad",
      without: base.rejected ? "rejected" : "approved", without_ms: base.ms, without_note: base.note,
      with: base.rejected || kv === "rejected" ? "rejected" : kv, kritic: kv,
      rejected_by: tools.filter((t) => t.approval === false).map((t) => t.tool),
      kritic_ms: tools.reduce((s, t) => s + t.ms, 0),
      response_tokens: Math.round(tools.reduce((s, t) => s + (t.response_chars ?? 0), 0) / 4),
      full_result_tokens: Math.round(tools.reduce((s, t) => s + (t.full_result_bytes ?? 0), 0) / 4),
      tools,
    };
    results.push(row);
    console.error(`${c.id.padEnd(22)} ${variant.padEnd(4)} without=${row.without.padEnd(8)} with=${row.with.padEnd(10)} by=[${row.rejected_by.join(",")}] ${(row.kritic_ms / 1000).toFixed(1)}s`);
    rmSync(dir, { recursive: true, force: true, maxRetries: 3 });
  }
}
k.close();
server.close();

// ---- summary
const bad = results.filter((r) => r.defect);
const good = results.filter((r) => !r.defect);
const count = (rows, key, v) => rows.filter((r) => r[key] === v).length;
const summary = {
  date: new Date().toISOString().slice(0, 10),
  kritic_version: JSON.parse(execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], { cwd: root, encoding: "utf8" })).packages[0].version,
  gpu: status?.health?.device ?? null,
  cases: bad.length,
  defects_caught: { without: count(bad, "without", "rejected"), with: count(bad, "with", "rejected") },
  defects_no_verdict_with: count(bad, "with", "no-verdict"),
  clean_false_rejections: { without: count(good, "without", "rejected"), with: count(good, "with", "rejected") },
  clean_no_verdict_with: count(good, "with", "no-verdict"),
  median_kritic_s: median(results.map((r) => r.kritic_ms / 1000)),
  total_kritic_s: Math.round(results.reduce((s, r) => s + r.kritic_ms, 0) / 1000),
  response_tokens_total: results.reduce((s, r) => s + r.response_tokens, 0),
  full_result_tokens_total: results.reduce((s, r) => s + r.full_result_tokens, 0),
};
function median(v) {
  const s = [...v].sort((a, b) => a - b);
  return s.length ? Math.round(s[Math.floor(s.length / 2)] * 10) / 10 : 0;
}
mkdirSync(outDir, { recursive: true });
const file = join(outDir, `${summary.date}${only ? `-${only}` : ""}.json`);
// Temp project paths carry the user name: replace them before writing.
const tmp = tmpdir();
const scrub = (text) => [JSON.stringify(tmp).slice(1, -1), tmp.split("\\").join("/"), tmp].reduce((t, p) => t.split(p).join("<tmp>"), text);
writeFileSync(file, scrub(JSON.stringify({ summary, results }, null, 2)) + "\n");
console.log(JSON.stringify(summary, null, 2));
console.error(`wrote ${file}`);

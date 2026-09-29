// Agentic token benchmark: does an outside critic save tokens compared with an agent reviewing
// its own work?
//
// Every task (bench/agentic/tasks.mjs) runs as a real headless Claude Code session, once per arm:
//   none         implement and stop (no review asked for)
//   self         implement, then critique and test your own work until confident
//   kritic-local implement, then the kritic end-of-task review with local Kev
//   kritic-cloud the same with cloud Jev (skipped without a gateway key)
// The session's own usage (from `claude -p --output-format json`) is the cost; the hidden tests,
// run afterwards, are the only judge of correctness. Sessions are isolated: no user settings,
// CLAUDE.md, hooks or MCP servers besides kritic (for the kritic arms).
//
//   node bench/agentic/run.mjs [--claude <exe>] [--kritic <exe>] [--model sonnet] [--arms a,b]
//                              [--only <task id>] [--repeats 1] [--budget-usd 2] [--out bench/results]
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { TASKS } from "./tasks.mjs";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : fallback;
};
const root = decodeURIComponent(new URL("../..", import.meta.url).pathname).replace(/^\/([A-Za-z]:)/, "$1");
const win = process.platform === "win32";
const claude = arg("--claude", process.env.CLAUDE_EXE ?? "claude");
const kritic = arg("--kritic", process.env.KRITIC_EXE ?? join(homedir(), ".kritic", "bin", win ? "kritic.exe" : "kritic"));
const model = arg("--model", "sonnet");
const repeats = Number(arg("--repeats", "1"));
const budget = arg("--budget-usd", "2");
const only = arg("--only", null);
const outDir = arg("--out", join(root, "bench", "results"));
const python = ["python", "python3"].find((p) => spawnSync(p, ["--version"]).status === 0);
if (!python) throw new Error("python is required to run the tasks' tests");

const keyFile = process.env.KRITIC_CLOUD_KEY_FILE ?? join(process.env.KRITIC_HOME ?? join(homedir(), ".kritic"), "ai-gateway.key");
const hasCloudKey = !!process.env.AI_GATEWAY_API_KEY || existsSync(keyFile);
const allArms = ["none", "self", "kritic-local", "kritic-cloud"];
const arms = (arg("--arms", allArms.join(","))).split(",").filter((a) => a !== "kritic-cloud" || hasCloudKey || (console.error("skipping kritic-cloud: no gateway key (run `kritic key`)"), false));

const BASE = "Work only in the current directory. The project is Python; run tests with `python -m unittest`.";
const REVIEW = {
  none: "When the request is implemented, stop.",
  self: "Before you finish, critically review your own implementation against every requirement in the request, think of edge cases the existing test does not cover, test them, and fix what is wrong. Repeat until you are confident it is fully correct, then stop.",
  kritic: "When the request is implemented, run the kritic end-of-task review: call mcp__kritic__critic_score (paths = the files you changed, base_dir = the current directory, task = the request, revision = 0) and mcp__kritic__artifact_validate (the same paths, commands = [\"python -m unittest\"], use_model = false). If approval is false, fix only the issues you can name from its findings and re-run with revision = 1 (at most 2 revisions), then stop.",
};

function write(dir, files) {
  for (const [rel, text] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, rel)), { recursive: true });
    writeFileSync(join(dir, rel), text);
  }
}

function session(task, arm, dir) {
  const kriticArm = arm.startsWith("kritic");
  const mcp = { mcpServers: {} };
  if (kriticArm) {
    mcp.mcpServers.kritic = { type: "stdio", command: kritic, args: [], env: { KRITIC_KEV_BACKEND: arm === "kritic-cloud" ? "cloud" : "local" } };
  }
  const mcpFile = join(dir, "..", `${arm}-mcp.json`);
  writeFileSync(mcpFile, JSON.stringify(mcp));
  const tools = ["Read", "Edit", "Write", "Glob", "Grep", "Bash", ...(kriticArm ? ["mcp__kritic__critic_score", "mcp__kritic__artifact_validate"] : [])];
  const prompt = `${task.prompt}\n\n${BASE} ${REVIEW[kriticArm ? "kritic" : arm]}`;
  const t0 = Date.now();
  const r = spawnSync(claude, [
    "-p", prompt, "--model", model, "--output-format", "json",
    "--setting-sources", "project", "--strict-mcp-config", "--mcp-config", mcpFile,
    "--allowedTools", tools.join(","), "--max-budget-usd", budget,
  ], { cwd: dir, encoding: "utf8", timeout: 30 * 60_000, maxBuffer: 64 << 20 });
  let out = {};
  try {
    out = JSON.parse(r.stdout);
  } catch {
    out = { is_error: true, result: (r.stderr || r.stdout || String(r.error)).slice(0, 500) };
  }
  const u = out.usage ?? {};
  return {
    ok: !out.is_error,
    error: out.is_error ? String(out.result ?? "").slice(0, 300) : undefined,
    turns: out.num_turns ?? null,
    input_tokens: u.input_tokens ?? 0,
    output_tokens: u.output_tokens ?? 0,
    cache_creation_tokens: u.cache_creation_input_tokens ?? 0,
    cache_read_tokens: u.cache_read_input_tokens ?? 0,
    cost_usd: out.total_cost_usd ?? null,
    wall_s: Math.round((Date.now() - t0) / 1000),
  };
}

function hiddenTests(task, dir) {
  writeFileSync(join(dir, "test_hidden_zz.py"), task.hidden);
  const r = spawnSync(python, ["-m", "unittest", "-v", "test_hidden_zz"], { cwd: dir, encoding: "utf8", timeout: 120_000 });
  const text = `${r.stdout}\n${r.stderr}`;
  const ran = Number(/Ran (\d+) test/.exec(text)?.[1] ?? 0);
  const failed = Number(/failures=(\d+)/.exec(text)?.[1] ?? 0) + Number(/errors=(\d+)/.exec(text)?.[1] ?? 0);
  return { passed: r.status === 0 && ran > 0, ran, failed };
}

const results = [];
for (const task of TASKS.filter((t) => !only || t.id === only)) {
  for (let rep = 0; rep < repeats; rep++) {
    for (const arm of arms) {
      const parent = mkdtempSync(join(tmpdir(), `kritic-agentic-${task.id}-`));
      const dir = join(parent, "work");
      write(dir, task.files);
      execFileSync("git", ["init", "-q"], { cwd: dir });
      execFileSync("git", ["-c", "user.name=bench", "-c", "user.email=bench@example.test", "add", "-A"], { cwd: dir });
      execFileSync("git", ["-c", "user.name=bench", "-c", "user.email=bench@example.test", "commit", "-q", "-m", "base"], { cwd: dir });
      process.stderr.write(`${task.id} #${rep + 1} ${arm} ... `);
      const s = session(task, arm, dir);
      const h = hiddenTests(task, dir);
      process.stderr.write(`${h.passed ? "PASS" : "FAIL"} ${s.output_tokens} out, ${s.turns} turns${s.error ? ` (${s.error.slice(0, 80)})` : ""}\n`);
      results.push({ task: task.id, rep, arm, ...s, hidden: h });
    }
  }
}

const version = spawnSync(kritic, ["--version"], { encoding: "utf8" }).stdout?.trim();
const file = join(outDir, `agentic-${new Date().toISOString().slice(0, 10)}.json`);
mkdirSync(outDir, { recursive: true });
writeFileSync(file, JSON.stringify({ date: new Date().toISOString(), model, kritic: version, arms, results }, null, 2));
console.log(file);

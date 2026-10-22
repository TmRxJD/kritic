// locate benchmark: does kritic's candidate identification find the files a real change touched?
//
// Ground truth is a repository's own history. For each commit that changes 1-8 source files, the
// commit subject is the task and the files it changed are the answer; the repository is checked
// out at the commit's parent so the change does not exist yet. The tool sees only the task text.
//
// Compared per commit:
//   keyword   the lexical prefilter alone (what grep-style search finds)
//   locate    Kev's ranking blended with it (the shipped tool)
// Metrics: recall@k (share of changed files in the top k), hit@k (any changed file in the top k),
// and the tokens a frontier model would read to inspect the top k files (chars / 3.6), against
// reading the whole repository's source.
//
//   node bench/locate/run.mjs --repo <path> [--repo <path> ...] [--limit 25] [--k 5] [--exe <kritic>]
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { connect } from "../mcp.mjs";

const args = process.argv.slice(2);
const opt = (n, d) => { const i = args.lastIndexOf(n); return i >= 0 ? args[i + 1] : d; };
const repos = args.flatMap((a, i) => (a === "--repo" ? [args[i + 1]] : []));
const limit = Number(opt("--limit", 25));
const K = Number(opt("--k", 5));
const exe = opt("--exe", join(homedir(), ".kritic", "bin", process.platform === "win32" ? "kritic.exe" : "kritic"));
if (!repos.length) throw new Error("give at least one --repo");

const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8", maxBuffer: 64 << 20 }).trim();
const CODE = /\.(rs|ts|tsx|js|jsx|mjs|py|go|java|kt|cs|c|h|cpp|swift|rb|php|lua|gd|svelte|vue|html|css|scss|sql|sh|ps1|gleam)$/i;
const NOISE = /(^|\/)(node_modules|dist|build|target|vendor|corpus|samples|calibration|fixtures?)\//i;
const tokens = (chars) => Math.round(chars / 3.6);

const kritic = await connect(exe);
const rows = [];
for (const repo of repos) {
  const log = git(repo, "log", "--no-merges", "--format=%H%x09%s", "-n", "400").split("\n").map((l) => l.split("\t"));
  let done = 0;
  for (const [sha, subject] of log) {
    if (done >= limit) break;
    const parents = git(repo, "rev-list", "--parents", "-n", "1", sha).split(" ").slice(1);
    if (parents.length !== 1) continue;
    const changed = git(repo, "diff", "--name-only", "--diff-filter=AM", parents[0], sha).split("\n").filter((f) => f && CODE.test(f) && !NOISE.test(f));
    if (changed.length < 1 || changed.length > 8) continue;
    if (/^(merge|bump|release|rename the)/i.test(subject) || subject.length < 12) continue;
    // Check out the parent into a scratch dir (cheap: git archive).
    const dir = mkdtempSync(join(tmpdir(), "kritic-locate-"));
    try {
      execFileSync("tar", ["-x", "-C", dir], { input: execFileSync("git", ["-C", repo, "archive", "--format=tar", parents[0]], { maxBuffer: 1 << 30 }), stdio: ["pipe", "ignore", "ignore"] });
    } catch (e) {
      rmSync(dir, { recursive: true, force: true });
      continue;
    }
    // Files that did not exist at the parent (new files) cannot be found: score the ones that did.
    const existing = changed.filter((f) => { try { return statSync(join(dir, f)).isFile(); } catch { return false; } });
    if (!existing.length) { rmSync(dir, { recursive: true, force: true }); continue; }
    const base = dir.split(String.fromCharCode(92)).join("/");
    const t0 = Date.now();
    const r = await kritic.call("locate", { task: subject, base_dir: base, top_k: 40, no_checklist: true });
    const res = r.result;
    if (res.error) { console.error("locate error:", res.error); rmSync(dir, { recursive: true, force: true }); continue; }
    const byKev = res.candidates.map((c) => c.path);
    const byKeyword = [...res.candidates].sort((a, b) => b.keyword - a.keyword).map((c) => c.path);
    const sizeOf = (p) => { try { return statSync(join(dir, p)).size; } catch { return 0; } };
    const hit = (list) => existing.filter((f) => list.slice(0, K).includes(f)).length;
    rows.push({
      repo, sha: sha.slice(0, 8), subject, changed: existing.length,
      keyword: hit(byKeyword), locate: hit(byKev),
      first_hit_keyword: byKeyword.findIndex((p) => existing.includes(p)) + 1 || null,
      first_hit_locate: byKev.findIndex((p) => existing.includes(p)) + 1 || null,
      read_tokens_topk: tokens(byKev.slice(0, K).reduce((n, p) => n + sizeOf(p), 0)),
      files_in_repo: res.files_in_repo, considered: res.files_considered, kev_ok: res.kev_ok, ms: Date.now() - t0,
    });
    process.stderr.write(`${repo.split(/[\\/]/).pop()} ${sha.slice(0, 7)} locate ${hit(byKev)}/${existing.length} keyword ${hit(byKeyword)}/${existing.length}  ${subject.slice(0, 60)}\n`);
    rmSync(dir, { recursive: true, force: true });
    done++;
  }
}
kritic.close();

const sum = (f) => rows.reduce((n, r) => n + f(r), 0);
const total = sum((r) => r.changed);
console.log(JSON.stringify({ k: K, commits: rows.length, files: total, rows }, null, 1));
console.error(`\n${rows.length} commits, ${total} changed files, k=${K}`);
console.error(`recall@${K}   keyword ${(sum((r) => r.keyword) / total * 100).toFixed(0)}%   locate ${(sum((r) => r.locate) / total * 100).toFixed(0)}%`);
console.error(`hit@${K}      keyword ${(rows.filter((r) => r.keyword > 0).length / rows.length * 100).toFixed(0)}%   locate ${(rows.filter((r) => r.locate > 0).length / rows.length * 100).toFixed(0)}%`);

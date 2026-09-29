// kritic plugin: TODO/FIXME/XXX markers added by the uncommitted change.
// Contract: one JSON object on stdin ({args, base_dir, out_dir}), one on stdout.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const input = JSON.parse(readFileSync(0, "utf8"));
const maxNew = Number(input.args?.max_new ?? 0);
const progress = (detail) => process.stderr.write(JSON.stringify({ event: "progress", detail }) + "\n");

let diff;
try {
  diff = execFileSync("git", ["diff", "HEAD", "--unified=0", "--no-color"], { cwd: input.base_dir, encoding: "utf8", maxBuffer: 1 << 26, stdio: ["ignore", "pipe", "pipe"] });
} catch (e) {
  console.log(JSON.stringify({ ok: false, error: `git diff failed: ${e.message}` }));
  process.exit(0);
}
progress(`scanning ${diff.length} bytes of diff`);

const findings = [];
let file = null, line = 0;
for (const l of diff.split("\n")) {
  if (l.startsWith("+++ ")) file = l.slice(6);
  else if (l.startsWith("@@")) line = Number(/\+(\d+)/.exec(l)?.[1] ?? 0);
  else if (l.startsWith("+")) {
    if (/\b(TODO|FIXME|XXX)\b/.test(l)) findings.push({ severity: "medium", message: `new marker: ${l.slice(1).trim().slice(0, 120)}`, file, line });
    line++;
  }
}
const approval = findings.length <= maxNew;
console.log(JSON.stringify({
  ok: true,
  approval,
  summary: `${findings.length} new TODO/FIXME/XXX marker(s), budget ${maxNew}`,
  findings,
  metrics: { new_markers: findings.length, budget: maxNew },
}));

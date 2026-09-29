// Summarise an agentic benchmark result as markdown:  node bench/agentic/report.mjs <results.json>
import { readFileSync } from "node:fs";

const data = JSON.parse(readFileSync(process.argv[2], "utf8"));
const rows = data.results.filter((r) => r.ok);
const errors = data.results.length - rows.length;
// Billed-equivalent tokens: fresh input + cache writes + output. Cache reads are listed apart
// (they cost about a tenth of fresh input).
const billed = (r) => r.input_tokens + r.cache_creation_tokens + r.output_tokens;
const mean = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : NaN);
const n = (x) => (Number.isFinite(x) ? Math.round(x).toLocaleString("en-US") : "—");

console.log(`Run ${data.date.slice(0, 10)}, model ${data.model}, ${data.kritic ?? "kritic"}; ${rows.length} sessions${errors ? `, ${errors} errored (excluded)` : ""}.\n`);
console.log("| Arm | Hidden tests passed | Output tokens / task | Billed tokens / task | Cache reads / task | Turns / task | Cost / task | Tokens per passing task |");
console.log("|---|---|---|---|---|---|---|---|");
for (const arm of data.arms) {
  const rs = rows.filter((r) => r.arm === arm);
  if (!rs.length) continue;
  const pass = rs.filter((r) => r.hidden.passed).length;
  const cost = mean(rs.map((r) => r.cost_usd ?? NaN));
  console.log(`| ${arm} | ${pass}/${rs.length} | ${n(mean(rs.map((r) => r.output_tokens)))} | ${n(mean(rs.map(billed)))} | ${n(mean(rs.map((r) => r.cache_read_tokens)))} | ${mean(rs.map((r) => r.turns ?? 0)).toFixed(1)} | ${Number.isFinite(cost) ? `$${cost.toFixed(3)}` : "—"} | ${pass ? n(rs.reduce((a, r) => a + billed(r), 0) / pass) : "—"} |`);
}
console.log("\nPer task (hidden tests, billed tokens):\n");
console.log(`| Task | ${data.arms.join(" | ")} |`);
console.log(`|---|${data.arms.map(() => "---").join("|")}|`);
for (const task of [...new Set(rows.map((r) => r.task))]) {
  const cells = data.arms.map((arm) => {
    const rs = rows.filter((r) => r.task === task && r.arm === arm);
    if (!rs.length) return "—";
    return rs.map((r) => `${r.hidden.passed ? "pass" : `fail ${r.hidden.failed}/${r.hidden.ran}`} ${n(billed(r))}`).join("; ");
  });
  console.log(`| \`${task}\` | ${cells.join(" | ")} |`);
}

// Render a benchmark result file as the markdown tables used in docs/benchmarks.md.
//   node bench/report.mjs bench/results/<date>.json
import { readFileSync } from "node:fs";

const { summary: s, results } = JSON.parse(readFileSync(process.argv[2], "utf8"));
const out = [];
const pct = (a, b) => `${a}/${b} (${Math.round((100 * a) / b)}%)`;

out.push(`Run ${s.date}, kritic ${s.kritic_version}, Kev on ${s.gpu}. ${s.cases} cases, each with a defective and a correct variant.`, "");
out.push("| | Without kritic (project tests) | With kritic (tests + prescribed review) |", "|---|---|---|");
out.push(`| Defects caught | ${pct(s.defects_caught.without, s.cases)} | ${pct(s.defects_caught.with, s.cases)} |`);
out.push(`| Correct changes rejected | ${pct(s.clean_false_rejections.without, s.cases)} | ${pct(s.clean_false_rejections.with, s.cases)} |`);
out.push(`| No verdict (a tool couldn't run) | — | ${s.defects_no_verdict_with + s.clean_no_verdict_with} of ${2 * s.cases} runs |`);
out.push(`| Review time per change | — | median ${s.median_kritic_s} s (total ${s.total_kritic_s} s for ${2 * s.cases} reviews) |`);
out.push(`| Tokens returned to the agent | — | ${s.response_tokens_total.toLocaleString("en-US")} for ${2 * s.cases} reviews (~${Math.round(s.response_tokens_total / (2 * s.cases))} per review) |`, "");

// Deterministic tools vs model-based ones (Kev critic, vision scores).
const MODEL = new Set(["critic_score", "visual_critic"]);
const layer = (pred) => {
  const rej = (r) => r.tools.some((t) => pred(t.tool) && t.approval === false);
  return [results.filter((r) => r.defect && (r.without === "rejected" || rej(r))).length, results.filter((r) => !r.defect && (r.without === "rejected" || rej(r))).length];
};
const [detBad, detGood] = layer((t) => !MODEL.has(t));
const [modBad, modGood] = layer((t) => MODEL.has(t));
out.push("By layer, each added to the project's tests:", "");
out.push("| Layer | Defects caught | Correct changes rejected |", "|---|---|---|");
out.push(`| Tests only (without kritic) | ${pct(s.defects_caught.without, s.cases)} | ${pct(s.clean_false_rejections.without, s.cases)} |`);
out.push(`| + deterministic kritic tools (parsers, scanners, Playwright, metrics, sampler) | ${pct(detBad, s.cases)} | ${pct(detGood, s.cases)} |`);
out.push(`| + model-based kritic tools (Kev critic, vision scores) | ${pct(modBad, s.cases)} | ${pct(modGood, s.cases)} |`);
out.push(`| + everything (the prescribed review) | ${pct(s.defects_caught.with, s.cases)} | ${pct(s.clean_false_rejections.with, s.cases)} |`, "");

// Which layer caught what, and which layer rejected correct code.
const tools = [...new Set(results.flatMap((r) => r.tools.map((t) => t.tool)))];
out.push("By tool: defects it rejected, and correct changes it rejected (of the cases where it ran).", "");
out.push("| Tool | Defects rejected | Correct changes rejected |", "|---|---|---|");
for (const t of tools) {
  const ran = (defect) => results.filter((r) => r.defect === defect && r.tools.some((x) => x.tool === t));
  const rej = (rows) => rows.filter((r) => r.tools.find((x) => x.tool === t).approval === false).length;
  const bad = ran(true), good = ran(false);
  out.push(`| \`${t}\` | ${rej(bad)}/${bad.length} | ${rej(good)}/${good.length} |`);
}
out.push("");

out.push("Per case:", "");
out.push("| Case | Defect planted | Without | With | Rejected by | Correct variant: with |", "|---|---|---|---|---|---|");
const byCase = new Map();
for (const r of results) byCase.set(r.case, { ...(byCase.get(r.case) ?? {}), [r.variant]: r });
for (const [id, { bad, good }] of byCase) {
  const fp = good.with === "approved" ? "approved" : `**${good.with}** by ${good.rejected_by.join(", ") || "—"}`;
  out.push(`| \`${id}\` | ${bad.category} | ${bad.without} | ${bad.with} | ${bad.rejected_by.join(", ") || "—"} | ${fp} |`);
}
out.push("");

// Compaction: what the agent received vs the full reports written to disk.
const withFull = results.flatMap((r) => r.tools).filter((t) => t.full_result_bytes > 0);
if (withFull.length) {
  const resp = withFull.reduce((a, t) => a + t.response_chars, 0) / 4;
  const full = withFull.reduce((a, t) => a + t.full_result_bytes, 0) / 4;
  out.push(`Tools that write a \`full_result\` file (${withFull.length} calls) returned ~${Math.round(resp).toLocaleString("en-US")} tokens to the agent; the full reports on disk hold ~${Math.round(full).toLocaleString("en-US")} tokens.`);
}
console.log(out.join("\n"));

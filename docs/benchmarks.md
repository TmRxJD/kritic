# Benchmarks: with vs without kritic

## What is measured

The question an agent faces at the end of a task is whether the change is ready to report as done.
The benchmark answers it for 14 realistic changes, twice each:

- a **defective variant** with one planted problem of a kind agents actually ship: a hard-coded
  credential, SQL built from input, `shell=True`, a known-vulnerable dependency, a stub left
  `not implemented`, a comment telling AI reviewers to approve, an off-by-one, a broken JSON config,
  leftover merge markers, a broken UI, a messy layout, a 24-branch function, an unbounded cache;
- a **correct variant** that does the same job properly. This measures false rejections, which cost
  the agent a revision round and the user's trust.

Each case is a small real project (a git repo with a committed base) plus the change, uncommitted.

- **Without kritic** = the project's own test command (`node --test` / `python -m unittest`), which
  an agent already runs before saying "done". Where the change has no automated check (the UI
  cases), this is "approved".
- **With kritic** = the same tests plus the review the kritic skill prescribes for that kind of
  change: `critic_score(git_diff)` and `artifact_validate(commands=[tests])` always, plus
  `security_critic` for secrets, input handling and dependencies; `ui_test` and `visual_critic` for
  web UI; `code_quality_critic` for new modules; `resource_critic` for long-running code.

A change counts as rejected when any tool returns `approval: false`. `null` (a tool that could not
run) is counted separately, never as a catch or a pass. Every case passes its own tests in both
variants except `logic-bug-tested`, whose test catches the bug. That is the usual situation: tests
are green, and the question is what they don't cover.

Reproduce with `node bench/run.mjs` (cases in [`bench/cases.mjs`](../bench/cases.mjs); raw results
with every tool's verdict, timing and evidence in [`bench/results/`](../bench/results/)), then
`node bench/report.mjs bench/results/<date>.json`.

## Results

Run 2026-09-29, kritic 0.4.0, Kev on cuda:0. 14 cases, each with a defective and a correct variant.

| | Without kritic (project tests) | With kritic (tests + prescribed review) |
|---|---|---|
| Defects caught | 1/14 (7%) | 14/14 (100%) |
| Correct changes rejected | 0/14 (0%) | 7/14 (50%) |
| No verdict (a tool couldn't run) | — | 0 of 28 runs |
| Review time per change | — | median 7.6 s (total 370 s for 28 reviews) |
| Tokens returned to the agent | — | 17,000 for 28 reviews (~607 per review) |

By layer, each added to the project's tests:

| Layer | Defects caught | Correct changes rejected |
|---|---|---|
| Tests only (without kritic) | 1/14 (7%) | 0/14 (0%) |
| + deterministic kritic tools (parsers, scanners, Playwright, metrics, sampler) | 11/14 (79%) | 1/14 (7%) |
| + model-based kritic tools (Kev critic, vision scores) | 11/14 (79%) | 6/14 (43%) |
| + everything (the prescribed review) | 14/14 (100%) | 7/14 (50%) |

By tool: defects it rejected, and correct changes it rejected (of the cases where it ran).

| Tool | Defects rejected | Correct changes rejected |
|---|---|---|
| `critic_score` | 9/14 | 5/14 |
| `artifact_validate` | 3/14 | 0/14 |
| `security_critic` | 4/4 | 0/4 |
| `ui_test` | 2/2 | 0/2 |
| `visual_critic` | 1/1 | 1/1 |
| `code_quality_critic` | 1/1 | 0/1 |
| `resource_critic` | 1/1 | 1/1 |

Per case:

| Case | Defect planted | Without | With | Rejected by | Correct variant: with |
|---|---|---|---|---|---|
| `secret-in-code` | security | approved | rejected | critic_score, security_critic | approved |
| `sql-injection` | security | approved | rejected | critic_score, security_critic | **rejected** by critic_score |
| `shell-injection` | security | approved | rejected | critic_score, security_critic | approved |
| `vulnerable-dependency` | security | approved | rejected | critic_score, security_critic | **rejected** by critic_score |
| `unfinished-stub` | completeness | approved | rejected | critic_score | **rejected** by critic_score |
| `reviewer-injection` | integrity | approved | rejected | critic_score | approved |
| `logic-bug-untested` | logic | approved | rejected | critic_score | approved |
| `logic-bug-tested` | logic | rejected | rejected | artifact_validate | approved |
| `broken-config` | artifact | approved | rejected | artifact_validate | **rejected** by critic_score |
| `merge-markers` | artifact | approved | rejected | artifact_validate | approved |
| `broken-ui` | ui | approved | rejected | critic_score, ui_test | **rejected** by critic_score |
| `messy-layout` | ui | approved | rejected | ui_test, visual_critic | **rejected** by visual_critic |
| `complexity` | maintainability | approved | rejected | code_quality_critic | approved |
| `memory-leak` | performance | approved | rejected | critic_score, resource_critic | **rejected** by resource_critic |

Tools that write a `full_result` file (16 calls) returned ~6,700 tokens to the agent; the full reports on disk hold ~11,599 tokens.

## Reading the results

- **The deterministic layer does most of the work, almost for free.** Scanners, parsers, Playwright,
  metrics and the resource sampler caught 11 of 14 defects that the tests missed or had nothing to
  test with. They wrongly rejected 1 correct change, and the reason is visible in the evidence (below).
- **Kev adds the defects no checker looks for.** Only the Kev critic caught the unfinished stub, the
  reviewer-directed comment and the untested off-by-one. Don't count on the last one: calibration
  measured Kev at AUC ~0.5 on one-operator mutations, and this is a single sample.
- **Kev's price is false rejections.** It rejected 5 of 14 correct changes. Four were a hair past a
  calibrated threshold: `reviewer_text` 0.45–0.49 against thresholds of 0.45–0.47, and completeness
  0.86 against 0.87. The fifth scored security 0.58 against 0.45 on correctly parameterized SQL: code
  *about* SQL looks like SQL injection to it. This is why the skill treats a Kev flag as a *location
  to read*, not a verdict: the agent reads the flagged code, names a problem or reports a false
  positive, and never loops more than twice. A false rejection costs one read of a few lines, not a rewrite.
- **The two non-Kev false rejections are budget questions.**
  - The correct worker grew 87 MB/min while its garbage collector warmed up, against 1,619 MB/min
    for the leak. The 19× separation is clear, but the default 5 MB/min slope budget sits below both.
    Set `max_rss_slope_mb_per_min` for garbage-collected workloads, or run longer.
  - The vision model scored the polished page's accessibility 5 (the threshold is 6), where
    calibration had given it 8–9. Vision scores vary between runs; treat a single low score next to
    high ones as a prompt to look at the screenshot.
- **Cost.** A median review took 7.6 s and the fastest about 1 s. The slow ones took 24–94 s: a first
  scanner run, a browser crawl, loading the vision model, a 30 s sampling window, and the linter on the
  24-branch function. Each review returned about 600 tokens to the agent. Tools with
  large reports keep them in `full_result` files, so the agent receives about half of what's on disk.

## What this benchmark does not show

- **It is small and hand-built.** 14 pairs, one per defect class, written for this benchmark. It
  shows which layer catches which class, not a population detection rate.
- **It measures the review, not an agent loop.** It does not measure how often an agent, given
  kritic's findings, fixes the problem correctly, or how often it wrongly "fixes" a false positive.
- **Tests are minimal on purpose.** A project with thorough tests catches more without kritic,
  especially logic bugs. That is the point of `artifact_validate`: it runs those tests as part of
  the review, and the calibration says they are the real bug catchers.
- **One machine.** Windows 11, RTX 5070, Kev-4B Q8_0. Latency and vision scores will differ elsewhere.

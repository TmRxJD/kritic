# Tools

Every tool returns JSON with a published output schema. `approval` is `true`, `false`, or `null`
when the tool could not reach a verdict (a dependency offline, partial coverage, a run that could
not finish). Approval is always computed in code from stated thresholds; Kev only triages and
scores, and a model opinion never removes a deterministic failure.

| Tool | Deterministic part | Kev / model part | Approval rule |
|---|---|---|---|
| `critic_score` | chunking (a truncated chunk is split and re-scored), git diff, missing files | per unit (spec `code_chunk` / `code_change`; the request is not included): defect, severity, security, unfinished, quality, reviewer text; once per call: completeness (`completeness` on code, `completeness_digest` on a digest) | only the specs' calibrated gates, at the family-wise threshold for the number of units (`status().gates` lists them); `null` if Kev is offline or coverage was partial |
| `artifact_validate` | JSON/YAML/TOML/Python-syntax parse, merge markers, bad UTF-8, missing files, **`commands` run verbatim** | optional critic (`use_model`) | any deterministic or command failure means `false` |
| `security_critic` | gitleaks (secrets), osv-scanner (vulnerable dependencies in any lockfile), semgrep (`p/default`), bandit (Python), cargo-audit (Rust); secrets are redacted | Kev `p_true_positive` per code finding with ±6 lines of context, for ordering only | any **high/critical** finding means `false`; a scanner that fails to run makes it `null`. `security_score` = 10 − 3·critical − 2·high − 0.5·medium |
| `code_quality_critic` | the project's linters (ruff, clippy, eslint, tsc when configured) plus metrics: function length, cyclomatic-style complexity, nesting, duplicated 8-line blocks | `p_hard_to_maintain` for the most complex functions → line-weighted `maintainability` | `false` on linter errors, or a function with complexity > 20 or more than 150 lines (both per call) |
| `ui_test` | Playwright crawl of a **local** app: clicks, fills, toggles and selects every visible control; page and console errors, failed requests, crashes, no-effect controls, `persist` selectors across reload, axe-core; screenshots; scripted `flows` | none | any breakage means `false`; serious/critical axe violations count unless `axe_blocking=false`; `ignore` exempts named controls |
| `visual_critic` | screenshot via Playwright (or `image_path`) | vision model: 0–10 layout clarity, spacing/alignment, hierarchy, contrast, accessibility, plus `unverified_observations` | `false` if any score < `vision.visual_min` (6) |
| `ux_critic` | the repo's `.kritic/ux.toml`; per screen: unlabelled controls, icon-only without tooltip, targets < 24 px, axe contrast, disabled without a reason; breadth-first exploration gives each mode and task its `optimal_steps`; `scattered` workflows | none for approval; a novice (vision model, Kev fallback) tries each task from its plain-words goal | `false` on a mode without a top-level entry, an unreachable, trivially-done or over-budget task, an error-level rule, or a primary task the vision novice fails 2 of 3 times. See [ux_critic](ux-critic.md) |
| `desktop_test` | Windows UI Automation: launches the app, exercises every enabled control (Invoke/Toggle/Value/Select, not the mouse), a Win32 watchdog records and closes dialogs and detects hangs; crashes, error dialogs with text, no-effect controls, stderr errors, `persist` controls across a relaunch, scripted `flows`. **Unity mode** (automatic when `UnityPlayer.dll` sits beside the exe): see [Unity](unity.md) | none | any breakage means `false` |
| `resource_critic` | launches a command (or attaches to a pid with `allow_attach`) and samples the process tree: CPU, RSS/private memory, handles, threads, disk I/O, whole-GPU use | leak-risk choice and `p_reasonable` for the described workload (reported; acts only if p(leak high) > 0.5) | `false` on budget violations. Defaults: memory growing > 5 MB/min over ≥ 15 s, handles +500, early exit; optional CPU/RSS/GPU caps |
| `hygiene_critic` | processes whose exe or command line is under the project roots, orphans (parent exited, PID reuse checked), listening sockets still bound after their owner exited, `leftover_globs` files; `clean=true` kills orphans and socket inheritors. `desktop_test` and `resource_critic` also report what each run left alive | none | any orphan, inherited socket, leftover or run leak means `false` |
| `unity_tests` | Unity Test Framework tests headless (`-batchmode -runTests`), editor version from `ProjectVersion.txt`, NUnit XML parsed | none | `false` on any failed test; `null` if nothing ran |
| `plugin_list` / `plugin_run` | the repo's own evaluators in `.kritic/plugins/*.toml` (JSON on stdin and stdout, progress on stderr) | optional per-plugin Kev questions on the plugin's summary and metrics | `false` if the plugin says so, on a high/critical finding (unless `findings_advisory`), or when a calibrated Kev gate fails; `null` on a contract error. See [plugins](plugins.md) |
| `map_eval` *(off by default)* | Beat Saber v2/v3/v4 difficulties: grid bounds, time order, duplicates, field types, cut directions, parity breaks (same swing within 1.5 beats; > 8% of notes fails), density curve | optional advisory `awkward` per 16-beat slice | any heuristic issue means `false` |
| `route` | — | choice: local worker / haiku / sonnet / opus | suggestion only |
| `context_trim` | budget, top-N | relevance per chunk | extractive only, no summaries |
| `token_metrics` | from the Claude Code session transcript: per-session and per-task tokens, context size, peak and growth per call, compactions, cache hit rate, output tokens/s; local model usage | none | — |
| `status` | laya health, resolved paths, config file and errors, every calibrated gate, built-in spec lint, event-stream URL, ready states | none | — |
| `spec_lint` | the full schema and lint rules for a decision spec (JSON or path), every error at once | none | — |

## Output size

`ui_test`, `desktop_test`, `security_critic`, `code_quality_critic`, `resource_critic` and
`ux_critic` write the complete result to `runs/<stamp>-<tool>/result.json` (`full_result`). The
MCP response keeps only problems and the top items (25 at most; `critic_score` flags at most 15),
so large reports stay out of the agent's context. The [benchmark](benchmarks.md) measures the
difference.

## Safety defaults

- `ui_test`, `visual_critic` and `ux_critic` refuse non-local URLs unless `allow_remote=true`.
- Controls that look destructive ("Delete", "Remove", "Reset"…) are skipped unless `allow_destructive=true`.
- `resource_critic` attaches to an existing process only with `allow_attach=true`.
- `security_critic` never echoes a secret; findings carry a redacted match.
- Critic input is treated as data: text in the code addressed to a reviewer is itself a finding
  (`reviewer_text`), never an instruction.

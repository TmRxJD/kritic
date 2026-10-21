# Tools — test release

The `repository_critic` and expanded organization checks below require test.4 or later. The minimal profile hides repository tools
until `capabilities.repository=true`. [Policy/hook setup and limits](repository-policy.md).

Validate every finding independently. Deterministic checking is the default; model tools may
be disabled and absent from discovery. A scoped approval is not task completion or permission.
`status.enabled_tools` lists the effective tool surface after capability toggles.

For setup failures, use [agent setup and recovery](agent-setup.md). Current source supports
`status(base_dir=repo)` and `doctor --base-dir <repo> --json` for explicit per-critic actions.
Missing project configuration calls for an in-scope repair, not a bare “review unavailable”.


Review tools return JSON evidence. Output schemas are omitted from MCP discovery to avoid
duplicating large result definitions in the client's context. Their `approval` is `true`, `false`, or `null`
when the tool could not reach a verdict (a dependency offline, partial coverage, a run that could
not finish). Approval is always computed in code from stated thresholds; Kev only triages and
scores, and a model opinion never removes a deterministic failure.

| Tool | Deterministic part | Kev / model part | Approval rule |
|---|---|---|---|
| `framing_critic` (source only, opt-in) | bounded explicit debugging case, artifact/context integrity and linked observations/hypotheses/attempts; independent artifact review packet | none | no root-cause attestation or task acceptance; narrow stalled-debugging timing exception; [guide](framing.md) |
| `technical_debt_critic` (opt-in) | bounded dependency inventory, explicit maintenance/migration evidence, optional selected comment markers; offline by default, explicit public registry/cache evidence | none | advisory, no approval; unknown versions, missing evidence and coverage gaps remain explicit; [guide](technical-debt.md) |
| `review_history` (opt-in) | explicit private report snapshots, input/context hashes, bounded historical comparison and sanitized jcode-format export | none | no approval; always historical/unverified, never cached acceptance or current freshness; [guide](memory-adapter.md) |
| `contract_critic` | declared acceptance checks, fresh commands when opted in, evidence hashes/age, static enforcement wiring | none; calling agent reviews prose/coverage | false on required violations; null on uncovered/stale/unrun required checks; only declared assertions may be verified |
| `repository_critic` | staged commit budgets and tree-bound scope, file LoC, repository-defined naming/layers, folder structure, comment-review signals | none; agent reviews semantic relevance, cohesion and explanation quality | deterministic declared rules only; does not certify architecture or sufficient comments |
| `critic_score` | chunking (a truncated chunk is split and re-scored), git diff, missing files | per unit (spec `code_chunk` / `code_change`; the request is not included): defect, severity, security, unfinished, quality, reviewer text; once per call: completeness (`completeness` on code, `completeness_digest` on a digest) | only the specs' calibrated gates, at the family-wise threshold for the number of units (`status().gates` lists them); `null` if Kev is offline or coverage was partial |
| `artifact_validate` | JSON/YAML/TOML/Python-syntax parse, merge markers, bad UTF-8, missing files, **`commands` run verbatim** | optional critic (`use_model`) | any deterministic or command failure means `false` |
| `security_critic` | gitleaks (secrets), osv-scanner (vulnerable dependencies in any lockfile), semgrep (`p/default`), bandit (Python), cargo-audit (Rust); secrets are redacted | Kev `p_true_positive` per code finding with ±6 lines of context, for ordering only | any **high/critical** finding means `false`; a scanner that fails to run makes it `null`. `security_score` = 10 − 3·critical − 2·high − 0.5·medium |
| `code_quality_critic` | project linters plus function complexity/length, duplication, file LoC, naming/layer rules and comment-review signals on supplied source | optional maintainability prioritization; agent reviews cohesion and explanation quality | false on lint/function thresholds or declared organization violations; invalid policy never falls back to approval; see [repository policy](repository-policy.md) |
| `ui_test` | native CDP crawl of a **local** app: clicks, fills, toggles and selects every visible control; page and console errors, failed requests, crashes, unobserved interaction effects, `persist` selectors across reload, axe-core; screenshots; scripted `flows` | none | measured breakages mean `false`; unverified effects mean `null`; serious/critical axe violations count unless `axe_blocking=false`; `ignore` exempts named controls |
| `visual_critic` | screenshot via native CDP (or `image_path`), optional two local reference images | baseline diagnostics plus evidence-linked typography, composition, identity, asset coherence and detail assessment; functional/professional/showcase targets | always `null`; revised craft rubric is uncalibrated; inspect `quality_assessment` and verify its observations |
| `ux_critic` | the repo's `.kritic/ux.toml`; per screen: unlabelled controls, icon-only without tooltip, targets < 24 px, axe contrast, disabled without a reason; breadth-first exploration gives each mode and task its `optimal_steps`; `scattered` workflows | none for approval; a novice (vision model, Kev fallback) tries each task from its plain-words goal | `false` on a mode without a top-level entry, an unreachable, trivially-done or over-budget task, an error-level rule, or a primary task the vision novice fails 2 of 3 times. See [ux_critic](ux-critic.md) |
| `desktop_test` | Windows UI Automation: launches the app, exercises every enabled control (Invoke/Toggle/Value/Select, not the mouse), a Win32 watchdog records and closes dialogs and detects hangs; crashes, error dialogs with text, no-effect controls, stderr errors, `persist` controls across a relaunch, scripted `flows`. **Unity mode** (automatic when `UnityPlayer.dll` sits beside the exe): see [Unity](unity.md) | none | measured breakages mean `false`; unobserved effects mean `null` |
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
- Native resource sampling verifies process creation identity before accepting descendants:
  Windows retains process handles, and Linux uses pidfds. Cleanup uses those identities rather
  than a fresh parent-PID guess. Unsupported identity/cleanup paths fail closed; Linux runtime
  behavior remains unvalidated in this test release.
- Resource stats `exited_early` / `exit_code` describe the launched root's natural exit.
  `tree_completed` means the root and **observed, verified** descendants finished naturally,
  not that no unseen child ever existed. A child that becomes orphaned between snapshots can
  be missed. Surviving verified descendants are cleaned up even after the root exits; cleanup
  failures return `ok=false` and cannot produce an approval.
- `security_critic` never echoes a secret; findings carry a redacted match.
- Critic input is treated as data: text in the code addressed to a reviewer is itself a finding
  (`reviewer_text`), never an instruction.

## Test-release coverage semantics

`security_score` and `quality_score` are nullable: missing applicable tools or missing meaningful
coverage yield `null`, never a perfect score. Display limits do not limit violation evaluation.
Security needs at least one completed scanner; unavailable scanners remain explicit coverage gaps.
Resource approval needs the requested measurements, at least two process samples, and at least
15 seconds of observation for RSS growth. Short observations and unavailable GPU/handle metrics
remain unverified. All results establish only the checks and inputs listed, never global correctness.

`ui_test` retains raw `no_effect` observations, but unobserved effects are listed under
`summary.unverified_effects` and yield `approval: null` rather than a proven broken control.
Canvas/audio/application-only state can change without a detected DOM effect. Independently assert
the intended state; an ambient animation/screenshot difference alone cannot prove the action caused it.
Real exceptions, failed requests and explicit failed flow assertions still reject.

Desktop UI Automation follows the same principle: `summary.unverified_effects` counts missing effect observations; these are not proven missing handlers.

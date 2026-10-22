# Agent setup and recovery

Use this when a critic is absent, unconfigured, or cannot collect evidence. Diagnose and repair
ordinary setup within the authorized task; do not merely relay an OS error to the user. Ask only
when the target, real requirements, credentials, paid/network disclosure or resource permission
cannot be determined safely. Do not ask the user to invent a contract when the repository already
defines the relevant acceptance commands and rules.

## Start with scope and effective configuration

1. Resolve the actual repository root from the current task/tool request. Do not use the skill's
   directory, the Kritic source checkout or a scratch chat directory as the reviewed project.
2. On initial setup or a setup failure, call `status(start=false, base_dir=repo)` when supported.
   `project_setup.tools` gives each
   tool's capability, default project path, presence state and recovery action. Missing
   `config_errors` means an empty list in current binaries; it is separate from `spec_errors`.
   If an older binary omits `project_setup`, use this reference and its supported `status` and
   `doctor --json` rather than repeatedly sending unsupported arguments.
3. With shell access, use that same configured executable for
   `kritic doctor --base-dir <repo> --json`. On older releases, use `doctor --json` plus the
   project checks below. Read `checks[].fix`. These are diagnostics, not execution approval.
4. Select critics relevant to the project and requested work. A disabled optional critic is
   not an installation error; a Rust game does not need Unity, and model-free work does not
   need local weights, vision or a new API subscription. `present_unvalidated` means only that
   a prerequisite path exists; the actual critic must parse it and collect real evidence.

Read the matching `examples/kritic.toml`/documentation before editing global configuration.
Use supported keys only; preserve unrelated settings, credential references, GPU UUIDs and
externally managed services. Back up existing configuration. Unknown keys are errors, not a
reason to disable strict validation. Reconnect the MCP connection after binary/global-config
changes; active processes cache that configuration. Project manifests are reread per call.

For a genuinely new CPU-only installation, `kritic setup --backend off --no-vision --no-register`
provisions deterministic dependencies without a model; use `--no-scanners` or `--no-harness`
when those groups are irrelevant. Setup may download packages. **Do not run this command over
an existing model-enabled configuration blindly.** Inspect `setup --help`, the current settings
and doctor fixes first; preserve the user's chosen backends and external ownership.

## Repair a missing project contract

`contract_critic` needs a project contract, not a global switch or a model. Its `base_dir` and
`contract_path` identify the exact attempted scope. When `setup.code=contract_setup_required`:

- Check the supplied root and look for an existing in-repository contract at another path;
  use `contract_path` if that is authoritative. Never borrow another project's contract.
- Read this repository's AGENTS.md, ownership/schema rules, build/test manifests, hooks/CI and
  the user's current acceptance criteria. Create `.kritic/contract.toml` if absent, preserving
  existing requirements if repairing one. The format example is not a ready-made project policy.
- Use `version=1`, a meaningful `title`, attributed `sources`, and required requirements with
  unique IDs, precise descriptions, real `checks` and actual `enforcement` where required.
  Use the project's canonical aggregate validation command where available. Never replace it
  with `echo`, file existence, broad regexes, zero tests or an all-optional manifest.
- Commands use `kind="command"` and `command="<actual project command>"`. Hook/CI bindings
  use `path` and a precise `pattern`; `require_enforcement=true` requires that wiring. Static
  wiring is not proof a hook executed, CI passed, or branch protection exists.
- Preserve semantic/manual obligations as explicitly unverified when no meaningful executable
  evidence exists. Do not delete them or invent an attestation just to produce approval.
- Rerun with `run_commands=false`, inspect parsing, sources and coverage, then execute reviewed
  commands only within authorization and resource limits. A skipped command is unverified.
  Inspect test discovery and effects; an exit code alone cannot establish the full task.

For example, if a repository already defines `cargo xtask verify`, references `OWNERS.toml`,
and wires `.githooks/pre-commit` to that command, bind those real requirements. Do not invent
`cargo test` as a substitute for the repository's broader gate. Do not overwrite game/app code
to repair missing reviewer configuration.

## Every critic's setup boundary

Paths below are defaults relative to the selected repository. Read matching source/package
documentation when a tool supports an explicit path override. Install only relevant dependencies.

| Tool | Setup / corrective action |
|---|---|
| `status` | Inspect configuration, effective tools, backend health and optional project setup. Keep `start=false` for diagnosis. External services remain externally owned. |
| `artifact_validate` | Supply real changed paths and reviewed project commands. Use `use_model=false` for deterministic validation. No project manifest or separate reviewer needed. |
| `contract_critic` | Author/repair `.kritic/contract.toml` as above; `capabilities.contracts=true`. Missing file means setup required, not a broken engine. |
| `staleness_critic` | `capabilities.staleness=true`; derive `.kritic/test-contracts.toml` from real canonical owners, explicit test corpus and reviewed input/test hashes. `kritic staleness --schema` is authoritative. `sync` is dry-run; `sync --write` projects only declared JSON fixtures. Never auto-bless review hashes or rewrite assertions to match broken code. See `docs/staleness.md`. |
| `invariant_critic` | `capabilities.invariants=true`; use the same test-contract manifest to declare key boundaries, rationale/failure behavior, release guards and mapped negative-test bindings. Textual presence is not proof that a guard executes or an invariant is sensible. Run real negative cases and independently review them. See `docs/staleness.md`. |
| `repository_critic` | Git plus relevant `.kritic/repository.toml` rules; `capabilities.repository=true`. Choose focused/broad/atomic deliberately. Hook installation is separate; preserve existing hooks and staged work. |
| `framing_critic` | `capabilities.framing=true`; author `.kritic/debug-case.json` using `kritic framing --schema`, actual observations and matching hashes/context. The example case is synthetic, not project evidence. |
| `review_history` | `capabilities.history=true`; explicitly select a full saved report, current input hashes, declared context and private storage. No model/indexing. Historical records never approve current work. |
| `technical_debt_critic` | `capabilities.debt=true`; real dependency manifests/locks. `.kritic/debt.toml` is optional for extra declarations/evidence. Default offline; registry requests need authorized `online=true`. |
| `critic_score`, `decide` | Optional authorized `kev.backend`/provider. Check configuration and health first. Local Kev needs a compatible running service; a separate cloud API needs existing credentials and disclosure authorization. A coding-client subscription does not imply an API key. |
| `visual_critic` | Optional `vision.backend` with an authorized image-capable provider, plus current image or reachable app/browser. Without vision, skip model scoring and inspect screenshots manually. Never fabricate visual approval. |
| `ui_test` | `capabilities.web=true`; Chromium browser and reachable local app with real routes/flows/persistence assertions. Configure `ui.browser` if discovery fails. Remote/destructive operations require explicit opt-ins. |
| `ux_critic` | `capabilities.ux=true`; `.kritic/ux.toml` from the matching web/Unity example, customized to real routes/selectors/tasks. A running supported app is required. Set `novice="off"` without a model. |
| `security_critic` | `capabilities.security=true`; install/configure gitleaks, osv-scanner, Semgrep and Bandit, plus cargo-audit for Rust. Inspect `scan.tools_bin`/`scan.cargo_tools_bin`; use `triage=false` without Kev. Missing scanners are gaps. |
| `code_quality_critic` | `capabilities.quality=true`; the project's lint/typecheck toolchain and real source paths, optional repository rules. Kritic does not supply the project toolchain. Use `use_kev=false` without a reviewer. |
| `desktop_test` | `capabilities.desktop=true`; Windows interactive desktop, Python 3.11+/uv and packaged harness. Set `desktop.uv`/`desktop.harness_py` as needed and supply the real executable/flows. Custom drawing can limit automation. |
| `unity_tests` | Only for Unity: desktop capability, matching installed Editor and real Test Framework tests. Resolve `ProjectSettings/ProjectVersion.txt`; set `unity.editor` if needed. Never install Unity for a non-Unity project. |
| `resource_critic` | `capabilities.resources=true`; authorized command or explicit PID attachment, workload, duration and measured budgets. No model needed for budget checks. Preserve shared processes and resources. |
| `hygiene_critic` | Resources capability, exact project roots and owned run identity. Start with `clean=false`; do not kill shared/unrelated processes to clear results. |
| `plugin_list`, `plugin_run` | `capabilities.plugins=true`; optional `.kritic/plugins/*.toml`, declared evaluator dependencies and actual selected plugin ID. Empty plugin lists are valid. Inspect execution before running. |
| `spec_lint` | Matching bundled specs. Fix genuine schema/spec defects with regressions; do not relax gates or invent calibration. |
| `token_metrics` | A supported session/transcript if available. Missing accounting is unavailable, not zero or precise billing. Keep private transcripts private. |
| `route`, `context_trim`, `locate` | Optional `capabilities.context=true`, selected reviewer and explicit text/query. Preserve agent-routing policy; never silently index a workspace. |
| `map_eval` | Optional `capabilities.domain_maps=true`; supported Beat Saber map content/path and metadata. Not a general game-map checker. Skip for unrelated formats. |

After a setup repair, rerun only the affected preflight/critic and the project's relevant real
validators. Report what was configured, what ran, what remains unverified and any exact blocker.
Do not stop at “review unavailable” when a missing project file or known dependency can be repaired
within the existing task. Do not turn setup recovery into permission to download models, change
providers, run arbitrary contract/plugin commands, or publish the user's project.

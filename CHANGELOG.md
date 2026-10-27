# Changelog

## 0.5.0-test.6 — test release

- Add the standalone Rust `kev` / `kev-filter` CLI: finite noninteractive command
  supervision, byte-exact raw stream archives, bounded extractive output and retrieval.
  Default filtering requires no model/GPU/MCP. Optional local Kev ranks ordinary stdout;
  it cannot run commands, generate summaries or alter the child's success/failure.
- Bound execution, capture and inference; expose clipping, fallback and incomplete
  evidence. Contain owned child processes with Windows Job Objects / Unix groups.
  HTTP ranking is restricted to numeric loopback endpoints without proxies/redirects.
- Include the filter in the Windows package and provide an Apple Silicon CLI-only
  archive, matching tracked source, versions and per-binary hashes. Run portable CLI
  checks on Linux and macOS as well as the Windows workspace checks.
- Include native Kev / MLX transport and explicit 0.8B Mac testing / larger production
  profiles, with pinned setup and measured M1 evidence. Configuration selection is
  separate from weight loading; smaller-model smoke tests do not establish 4B parity.
- Document installation, optional ranking, log privacy, resource/process boundaries,
  failure codes and coverage limits without increasing MCP tool-schema overhead.

## 0.5.0-test.5 — test release (2026-10-22)

- Optional model-free test-staleness and invariant review share one canonical ownership/test
  manifest, bounded inventory, reviewed hashes and explicit JSON fixture projection. Work-contract
  checks use the same graph. No automatic test blessing or semantic/execution attestation.

- Refresh certificate-verification dependencies and the optional desktop harness's Pillow
  dependency for newly published security advisories; the CPU benchmark also explicitly pins
  a patched certifi release for reused environments. No model download is required.

- Project-aware, read-only setup guidance for every tool through optional `status(base_dir)`
  and `doctor --base-dir`; scoped contract loading errors now include recovery steps. The
  packaged skill includes an offline setup reference rather than passing raw missing-file errors
  to users. Presence checks never establish readiness, execution or acceptance.

- Optional deterministic debugging-framing review and a local independent-review packet, with
  explicit artifact/context binding and linked causal evidence. Disabled by default; no root-cause
  attestation, automatic commands, network/indexing or model inference. The framing-only stalled-debugging
  exception leaves ordinary critics at end of task. Included in this release with matching setup documentation.

- Compact MCP discovery: short shared instructions and tool summaries; redundant schema
  titles/examples/empty defaults are omitted. All tools, input constraints, field descriptions,
  meaningful defaults and execution opt-ins remain available. No tools are disabled to save tokens.

- Correct Windows inventory path normalization, Cargo audit CVSS scoring and nested OSV
  manifest provenance. Hermes compression now bounds the whole request and enforces its
  absolute output cap.
- Present the governance workflow, hardware/capability boundaries and incremental setup in
  the README. Release packaging includes only tracked harness files, rejects stale staging,
  and records binary/source provenance with version/tag consistency checks.

## 0.5.0-test.4 — test release

- Optional, disabled-by-default technical-debt review: offline dependency inventory, pnpm v9
  workspace resolutions, explicit maintenance/migration notes and bounded comment-marker leads.
  Online public-registry evidence is separately authorized; urgency/effort remain advisory,
  with no automatic upgrades or compatibility guarantee.
- Corrected the MCP review-history output schema to declare object-shaped results; regression
  coverage guards client schema validation. The packaged MCP server uses the corrected schema.

- Optional, disabled-by-default review history: explicit reports and input/context hashes,
  bounded historical comparison and sanitized low-trust jcode-format export. No cached approval,
  automatic indexing, live jcode modification or demonstrated memory-savings claim.

- Model-free repository policy review and explicit staged-index hooks, with broad/focused/atomic
  commit budgets, tree-bound scope plans and declared naming/layer/import rules.
- Ordinary code-quality review includes physical/nonblank file sizes, oversized-file baselines
  and explanatory-comment review signals. Static compliance is not semantic architecture approval.
- Stricter provisional visual-quality rubric and an optional CPU semantic-map benchmark;
  no embedding candidate met the declared retrieval gate.
- Explicit external ownership of Kev/vision services prevents Kritic from starting, stopping
  or unloading services managed by the user or an external router. GPU routing stays with that owner.

## 0.5.0-test.3 — published test release

[Published release and packaged binaries](https://github.com/TmRxJD/kritic/releases/tag/v0.5.0-test.3).
Deterministic checks are the default; local/cloud reviewers remain optional. The release includes
the test-release skill, work contracts, capability configuration and client setup documentation.
New repository enforcement and expanded organization review are included in test.4 above.

## 0.5.0-test.1 — historical preparation (not published)

- Test-release skill: independently verify critic claims, evaluate workflow viability, reproduce
  and fix Kritic defects with regressions and upstream PRs.
- Deterministic-by-default setup/config; local models and cloud reviewers explicitly optional.
- OpenAI/OpenAI-compatible, Anthropic, Gemini and JEV transports; optional cloud vision,
  strict bounded responses, redacted errors, provider-origin-bound saved credentials.
- Work contracts: declared acceptance checks, coverage gaps, current input hashes, stale evidence,
  final revalidation, separately reported static enforcement wiring and prose sources.
- Capability/tool toggles, no GPU side effects for cloud/off, scoped calibration provenance,
  rejection of empty/skipped/vacuous approvals, synchronized docs and offline CI.

## 0.4.0 — 2026-09-29: kritic

First public release, under the name **kritic** ("k" for Kev). Earlier versions (0.1–0.3) were a
private, single-machine tool with the working name *jev_governance*.

- **Renamed** throughout: crate and binary `kritic`, MCP server `kritic`, environment variables
  `KRITIC_*`, repo folder `.kritic/` (plugins and `ux.toml`), skill `kritic`.
- **Config file**: `kritic.toml` (`$KRITIC_CONFIG`, else `$KRITIC_HOME/kritic.toml`) with every
  setting; environment variables still override it. `status` reports the file read and any errors.
- **No machine-specific defaults**: everything defaults under `~/.kritic` or to PATH lookups. laya is
  found in `~/.kritic/laya/` or one folder below; its log moved to the log directory.
- **`[tools] disabled`**: hide tools from the client. `map_eval` (Beat Saber) is off by default.
- **Install scripts**: `scripts/install.sh` (binary + harness + config) and
  `scripts/fetch-models.sh` (laya for CUDA/Vulkan/Metal, Kev-4B GGUF, Windows CUDA runtime DLLs).
- **Unity `ux_critic`**: `app.shot_dir` replaces an app-specific environment variable.
- **Calibration scripts** take `--corpus`, `--history` and `--maps` instead of fixed paths.
- **Benchmark** (`bench/`): 14 defect/clean case pairs reviewed without and with kritic.
- **Docs and examples**: `docs/`, `examples/` (config, Claude Code skill/agent/rule, a repo plugin,
  `ux.toml` for web and Unity).

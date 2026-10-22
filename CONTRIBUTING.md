# Contributing to the test release

Kritic's skill is a field-testing protocol. Verify findings independently. Fix reproducible Kritic
defects directly and submit focused PRs with regression coverage and sanitized evidence.

Keep commits scoped to one explainable purpose. Current source supports repository-owned
`broad`, `focused` (default), or `atomic` budgets and explicit Git hooks; follow
[the policy workflow](docs/repository-policy.md), review each staged hunk and bind an exact scope
plan. Never widen budgets or bulk-baseline files solely to make a gate pass. Local hooks are
bypassable; CI/review remain necessary. Record static violations separately from heuristic
recommendations and independently judge naming, cohesion, discoverability and comment quality.

History/format-export changes must keep historical evidence separate from current acceptance.
Use sanitized fixtures for report/input/context mismatch, bounded storage and private-data leakage.
Do not present a serialization test as live jcode integration or a memory-savings benchmark.

MCP schema changes need a server-construction/tool-discovery regression: every tool's
`inputSchema` must explicitly declare root `type: "object"`. Tagged enums can produce
alternatives without that root declaration; verify the generated schema and constructor,
not only a JSON serialization roundtrip. Include optional tools in the enabled test surface.

Technical-debt changes must preserve offline defaults, explicit online disclosure, private-source
skips, bounded requests/cache/corpus and visible lookup/coverage gaps. Test exact lock resolution
separately from requirements, workspace/alias handling, stale evidence and unsupported ordering.
Urgency/effort are advisory; no automatic update, inferred secret value or compatibility claim
belongs in the feature. [Evidence and validation workflow](docs/technical-debt.md).

1. Reproduce the defect and distinguish it from a bug in the user's project.
2. Add a regression that fails for the demonstrated behavior (avoid vacuous/mirrored assertions).
3. Fix the implementation without weakening checks, expanding approval scope or hiding coverage gaps.
4. Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and
   `cargo test --all-targets`. Run the relevant Python/legacy harness tests for changes there.
5. Record tests that actually ran, live checks skipped, and provider/OS coverage limits. Open a PR
   to https://github.com/TmRxJD/kritic with expected/actual behavior and evidence.

Default tests require no GPU/API keys. Live tests require explicit `KRITIC_LIVE=1`; do not opt in
without configured/authorized resources. Provider changes need malformed/refused/truncated response,
credential isolation, calibration spoofing, and no-local-side-effects regressions. Contract changes
need uncovered, stale, missing, failing and vacuous acceptance cases.

Deterministic evidence is scoped, not universal truth. Model judgments are advisory. Local calibrated
gates apply only to their measured identity; cloud self-reported probabilities never inherit them.
Empty/all-skipped coverage cannot approve a task. Reports and issue comments must not expose private
code, credentials, hidden prompts or machine-specific paths.

For canonical test-source/invariant changes, use the same `.kritic/test-contracts.toml` graph.
Run `kritic staleness check` and `kritic staleness invariants` plus real behavioral validators.
Fixture sync may project independent contract data; never auto-bless review hashes or replace
assertions with implementation output. Explain the intended behavior and review any changed pins.
Kritic's initial graph deliberately covers only its UX adapter and staleness regression test files;
unmapped runner/corpus/semantic coverage is unverified. Do not claim full-repository currency.

Project-specific evaluators belong in `.kritic/plugins/`. Do not change verdict thresholds without
recorded calibration and held-out evidence. Historical results are not current release validation.

Hermes regressions use `python -m pytest integrations/hermes/tests`, with `PYTHONPATH` pointing
at a real Hermes checkout and `KRITIC_EXE` set to the absolute built executable. CI pins the
Hermes revision in `.github/workflows/ci.yml`; only pytest is installed for these tests.
The optional legacy browser harness uses `npm ci && npm test` in `harness/` with Playwright
Chromium installed. Native browser tools do not require that legacy Node harness.

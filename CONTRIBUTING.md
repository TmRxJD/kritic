# Contributing to the test release

Kritic's skill is a field-testing protocol. Verify findings independently. Fix reproducible Kritic
defects directly and submit focused PRs with regression coverage and sanitized evidence.

Keep commits scoped to one explainable purpose. Current source supports repository-owned
`broad`, `focused` (default), or `atomic` budgets and explicit Git hooks; follow
[the policy workflow](docs/repository-policy.md), review each staged hunk and bind an exact scope
plan. Never widen budgets or bulk-baseline files solely to make a gate pass. Local hooks are
bypassable; CI/review remain necessary. Record static violations separately from heuristic
recommendations and independently judge naming, cohesion, discoverability and comment quality.

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

Project-specific evaluators belong in `.kritic/plugins/`. Do not change verdict thresholds without
recorded calibration and held-out evidence. Historical results are not current release validation.

Hermes regressions use `python -m pytest integrations/hermes/tests`, with `PYTHONPATH` pointing
at a real Hermes checkout and `KRITIC_EXE` set to the absolute built executable. CI pins the
Hermes revision in `.github/workflows/ci.yml`; only pytest is installed for these tests.
The optional legacy browser harness uses `npm ci && npm test` in `harness/` with Playwright
Chromium installed. Native browser tools do not require that legacy Node harness.

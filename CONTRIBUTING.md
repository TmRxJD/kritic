# Contributing

- **Facts come from code.** A new check produces findings deterministically; Kev may only score or
  rank them through a declared, linted [decision spec](docs/decision-specs.md). A model opinion never
  removes a deterministic failure.
- **Never fabricate.** A missing dependency makes a tool `failed`/`skipped` with approval `null`.
- **Gates need evidence.** A Kev question can fail approval only with dated calibration evidence
  produced by `scripts/calibrate-specs.mjs`. Don't hand-edit thresholds in `specs/`.
- **Project-specific checks are plugins** ([docs/plugins.md](docs/plugins.md)), not new tools.
- Before a PR: `cargo clippy --all-targets -- -D warnings`, the offline test suites (see
  [docs/extending.md](docs/extending.md#build-and-test)), and the live suites if you touched a
  model-backed path. If a change can move verdicts, rerun `node bench/run.mjs` and include the diff
  of `bench/results/`.

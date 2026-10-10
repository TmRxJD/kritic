# Test-release protocol

The objective is to evaluate the viability of Kritic's skill, tools, configuration and findings.
Do not inherently trust a critic, including a deterministic checker. Validate its inputs, scope,
test discovery, assertions, execution evidence and applicability. A correct parser check cannot
establish correct business logic; a model's opinion cannot establish a test was executed.

For each finding, record the reproducer, independent evidence, expected/actual result, Kritic
version, enabled capabilities, platform, provider/model (if any) and whether it helped the task.
Record false positives, false negatives, latency and workflow cost. Missing dependencies,
uncalibrated models and untested environments remain explicitly unverified.

## Vacuous checks to reject

- No tests discovered, zero required checks, all criteria skipped, or a requirement with no assertions.
- Existence/substring checks presented as proof behavior works; regexes that match everything.
- Stale reports, mismatched input hashes, fabricated screenshots/scores, unexecuted command claims.
- Thresholds borrowed from another provider/model/quantization or generated “confidence” treated
  as measured reliability.
- Assertions that merely repeat implementation constants, optional-only coverage presented as
  full task acceptance, or “CI configured” presented as “CI ran/branch protected”.

## Fix Kritic defects and contribute

Agents using this test-release skill should fix reproducible problems in Kritic directly:
locate/clone the Kritic checkout, create a focused branch, add a regression reproducer, fix the
underlying implementation, run meaningful checks, and open a PR to https://github.com/TmRxJD/kritic.
Do not bypass checks or weaken an expectation to obtain green output. Keep the user's target
project changes separate from Kritic fixes. A target-project bug is not automatically a Kritic bug.
Include sanitized evidence and explicit limits in the PR; no credentials/private user code.
Do not send unrelated messages or publish the target project. If repository access or credentials
prevent a PR, retain the verified patch/reproducer and report the precise blocker without claiming
the issue was fixed upstream.

Use [CONTRIBUTING.md](../CONTRIBUTING.md) for checks and the bug report/PR templates. A live API
or GPU test is optional and requires configured access; offline mocks are the baseline.

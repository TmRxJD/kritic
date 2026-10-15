# Test-release protocol

Use matching binary/configuration versions: repository policies/hooks, expanded organization
review, history and technical-debt tools require `v0.5.0-test.4` or later.
For repository tests, record `broad`/`focused`/`atomic`, explicit overrides, staged-tree identity,
inventory/exclusions and unresolved import coverage. Exercise stale plans, out-of-scope paths,
oversized changes, malformed policy and hook failure/bypass cases. A green static gate cannot
establish semantic cohesion or sufficient explanations. [Repository policy](repository-policy.md).

Optional history tests must preserve its boundary: historical reports never approve
current work. Check disabled-tool behavior, malformed/tampered records, corpus/path/size limits,
context mismatch, paginated metadata and truncated comparisons. Inspect exports for raw source,
report/context strings and local paths; a format fixture alone does not validate live jcode
import or memory savings. [Commands and limitations](memory-adapter.md).

Technical-debt tests must distinguish a declared requirement, a resolved lockfile version and
an observed registry default. Exercise offline misses, stale caches, private sources, failed
lookups, newer installed versions, qualifiers and the online batch deadline. Upgrade effort is
an estimate: validate exact-version migration notes, environment-variable names and actual
upgrade/test results before treating the estimate as demonstrated work. [Debt guide](technical-debt.md).

## Unreleased Windows test binaries

A CI run stages `kritic-windows-test` after its Rust checks pass. It contains `kritic.exe` and
`manifest.json`, with the PR head, tested checkout, run ID and executable SHA-256. Artifacts are
retained for three days. Choose the run for the source revision you intend to test and confirm
its checks passed; compare the executable hash with that manifest before using it.

This is a standalone Windows test executable, not the published release ZIP or a complete
optional-harness package. Use configuration/examples from the matching source revision and
the [installation guide](getting-started.md); the core needs no Rust installation, model or GPU.
For example, the hash can be checked with `Get-FileHash .\kritic.exe -Algorithm SHA256`.
Keep an existing working executable/configuration until the replacement has passed `doctor`
and an actual MCP initialization/tool-discovery test in your client.

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

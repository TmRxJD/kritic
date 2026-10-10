# Field evidence and known limits

Evidence audit: 2026-10-10. Tool approvals alone are not proof. Repository records are attributed
as historical records; only the explicitly rerun checks below are current independent evidence.
No private code, source payloads, credentials or machine-specific run paths are published here.

## Towerium: deterministic work with the local model disabled

The public [validation record](https://github.com/TmRxJD/towerium/blob/778febf06ef81a606829997fea8be59a2b52cbe2/VALIDATION.md)
records 64 controller tests executed through Kritic's Bash validator while Kev was off, and a
55-file deterministic review. It also records quoting/path mistakes, binary files skipped by text
parsing, and bounded UX crawls that did not reach every requested flow.

We freshly ran these real test files on 2026-10-10: `playtest-policy`, `human-controls`, `aim-assist`,
`auto-aim`, and `automation`: **78 passed, 0 failed, 0 skipped**. Their assertions cover bounded
manual aiming, shot/cooldown rules, targeting reservations, automatic aiming and purchase/countdown
behavior. See [the tests at the inspected revision](https://github.com/TmRxJD/towerium/tree/778febf06ef81a606829997fea8be59a2b52cbe2/tests).

This supports using Kritic to run deterministic project checks when inference is unavailable.
The suite also ran through the current `artifact_validate` MCP tool (`use_model=false`): its
command exited zero and approval was true, with the same 78-test TAP evidence. The actual
correctness evidence comes from the assertions, not Kritic's approval flag. It does not
establish full game balance, survival targets, mobile hardware or reviewer accuracy.

## Findings repaired before issue documentation

- Quoted Windows validator paths have dedicated regressions; previous field invocations failed
  with `is not recognized as an internal or external command`. Linux `/mnt/c` paths passed to a
  Windows Git process are still invalid input; this is not evidence that the project failed.
- Focus-only/scroll-only browser actions were reproduced as `no_effect` despite moving focus or
  scroll position. Native and legacy testers now capture those changes, exclude ordinary clicked
  button focus/automatic scrolling, and retain inert controls as unobserved. The native browser
  regression executed against Chromium; see [its test](../tests/native_effects.rs) and
  [the headless parity regression](../harness/test/effects.test.mjs).
- Unobserved effects previously became hard failures. They now populate
  `summary.unverified_effects` and leave approval unknown; measured exceptions/failed assertions
  still reject. See [verdict regressions](../tests/phase2.rs).
- Invalid/NaN visual thresholds and invalid scores cannot produce approval; bounds are validated.
- Missing scanners, malformed/partial scanner reports, zero scanned files, and absent process
  measurements now leave coverage unverified; partial findings remain visible.
- RustSec maintenance notices were mislabeled as vulnerabilities. Explicit unmaintained/notice
  metadata is informational; mixed or scored security advisories keep their security severity.
  [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html) is a maintenance notice.

## Proven remaining observation limit

A minimal canvas button paints blue pixels red. Independent pixel sampling confirms
`[255,0,0,255]`, while both browser testers report the raw interaction as `no_effect`. A separate
animated canvas changes pixels without an interaction, so screenshot differences alone cannot
prove a button caused an effect. Generic sensing cannot validate arbitrary application semantics.

The false hard-failure verdict is repaired. Richer explicit canvas/application-state assertions
remain [tracked in GitHub issue #1](https://github.com/TmRxJD/kritic/issues/1).
A [minimal synthetic page](../examples/canvas-observation.html) is included. Until then, independently assert intended state and treat this as
unverified coverage. Audio behavior has similar observation limits, but the specific audio
false-positive cause from historical Towerium notes was not independently reproduced here.

## Other inspected repositories: useful leads, limited proof

- AgenticCognitiveSubstrate records 32 tests plus deterministic security/quality checks while
  protecting another training job's GPU capacity. Its source contains real local HTTP engine
  contract tests. This checkout has no Git provenance; its suite was not rerun in this audit.
  We do not promote its old counts as current release validation.
- Beat Agent has a deterministic domain `map-score` plugin and four saved outputs with differing
  verdicts. The old raw-number/typed-band mismatch is absent from the current no-model manifest.
  Those outputs lack map-input identity, no calibration regression source was located, and this
  checkout has no remote. A fresh synthetic v3 map with identical beat/cell notes was rejected by the real plugin with
`1 notes share a cell and beat`, no model involved. That proves overlap detection only; its
historical scores are not advertised as verified mapping quality.
- Tower Game Hub has clean tracked validation notes and asset tests. Its direct-browser claims
  were not rerun here; accessibility warnings on browser-generated raw-image pages do not establish
  defects in the hub's own markup.
- TheImpossibleTower records model-free checks but explicitly lacks fresh browser coverage in its
  latest validation record. Old opening-flow evidence is not current game validation.

## Release verification boundaries

Mock provider tests verify request/response handling, isolation and rejection paths, not live
account compatibility or model quality. Local model/GPU behavior and non-Windows platforms remain
unverified. Semgrep can report parser/coverage errors; Kritic retains findings and does not turn
that into a clean security pass. Generic command-execution and loopback-WebSocket scanner warnings
need contextual review rather than blanket suppression. None of this is release-grade assurance.

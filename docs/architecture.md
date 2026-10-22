# Architecture — test release

Kritic connects repository expectations to evidence, independent agent review, selected enforcement
and fresh verification. Contracts and policies belong to the repository; the skill guides the
agent, while opt-in hooks and CI enforce only their configured checks.

The default Rust MCP server collects deterministic evidence. Its skill asks the calling agent
to validate that evidence and test Kritic's viability. Optional local/cloud reviewers provide
advisory judgments; cloud/off calls do not acquire GPU locks or unload local models.

- config.rs: environment/TOML defaults, explicit model providers and capability/tool toggles.
- server.rs: typed MCP routing and lifecycle events; disabled tools are not callable through MCP.
- tool_metadata.rs: concise discovery descriptions and schema-annotation compaction. Validation
  keywords, property names, references and field guidance are preserved; no tool is hidden by
  this optimization. Short shared instructions avoid repetition in clients that prefix each tool.
- checks.rs/tools.rs: parsing, requested validator execution, coverage gaps and artifact reports.
- contract.rs: acceptance assertions, hashed/fresh evidence, final revalidation and static wiring.
- staleness/: one canonical source/test/invariant manifest, bounded inventory and hash review,
  with explicit fixture projection. Changed hashes require review; sync never grants approval.
- provider.rs: explicit bounded HTTPS transports, native authentication, JSON-only compatibility.
- kev.rs/spec.rs/critic.rs: typed answers, strict decoding, per-client calibration identity, advisory
  unknown/different models. Calibration is no longer process-global.
- native/: browser CDP, process/resource/hygiene implementations; Python for Windows UI Automation.
- scan.rs/phase3.rs: external scanners/linters and measured findings; model triage cannot erase them.
- repository.rs/commit_policy/: repository-owned policy, tracked/staged inventories, index-bound
  scope plans and explicit local hooks; no semantic proof that every hunk serves its purpose.
- organization.rs/organization/: line budgets, declared naming/layers/import aliases and bounded
  comment/import heuristics. Ordinary code-quality review applies them to supplied files;
  `repository_critic` additionally offers the tracked inventory and staged commit gate.
- history.rs/history_api.rs/history/: opt-in private report snapshots, explicit input/context
  fingerprints and historical comparison; a separate sanitized jcode-format export. No model,
  workspace indexing, live jcode mutation or cached verification. [Memory adapter](memory-adapter.md).
- debt.rs/debt/: opt-in dependency/maintenance evidence. `inventory` reads only bounded manifests
  and explicit declarations; `registry` separates offline cached evidence from authorized HTTPS
  lookups; `ranking` estimates urgency/effort without applying upgrades; `maintenance` reads
  version-bound declarations and caller-selected comment markers. `cli/debt.rs` exposes the
  same capability through the CLI. No model, package-manager script or automatic update runs.
  [Privacy, coverage and budgets](technical-debt.md).
- vision.rs/ux.rs: optional local/cloud screenshot and novice judgments; deterministic workflow rules.
- framing.rs/framing/: optional bounded debugging-case validation, evidence integrity and causal
  relationship review. It produces a local artifact packet for independent scrutiny, without
  automatic command execution, network/indexing or model inference. A frame is not a proven
  root cause. [Case workflow and timing exception](framing.md).

GPU coordination applies only to explicitly selected local inference. One selected model at a
time is coordinated within/across Kritic processes; unrelated applications are outside that
coordination. If the configured wait expires, contention is reported. No global VRAM guarantee.
The prior measured 12 GB spillover explains the sequential design; it is not an 8 GB benchmark.

Run reports/logs/models default under KRITIC_HOME. They can contain private source/output: sanitize
before sharing. Contract reports are evidence ledgers, not cryptographic execution attestations.
External commands execute with the user's privileges, not inside a security sandbox.

Repository policy/hook functionality is included in `v0.5.0-test.4` and later. See [policy and coverage boundaries](repository-policy.md).

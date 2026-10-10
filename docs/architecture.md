# Architecture — test release

The default Rust MCP server collects deterministic evidence. Its skill asks the calling agent
to validate that evidence and test Kritic's viability. Optional local/cloud reviewers provide
advisory judgments; cloud/off calls do not acquire GPU locks or unload local models.

- config.rs: environment/TOML defaults, explicit model providers and capability/tool toggles.
- server.rs: typed MCP routing and lifecycle events; disabled tools are not callable through MCP.
- checks.rs/tools.rs: parsing, requested validator execution, coverage gaps and artifact reports.
- contract.rs: acceptance assertions, hashed/fresh evidence, final revalidation and static wiring.
- provider.rs: explicit bounded HTTPS transports, native authentication, JSON-only compatibility.
- kev.rs/spec.rs/critic.rs: typed answers, strict decoding, per-client calibration identity, advisory
  unknown/different models. Calibration is no longer process-global.
- native/: browser CDP, process/resource/hygiene implementations; Python for Windows UI Automation.
- scan.rs/phase3.rs: external scanners/linters and measured findings; model triage cannot erase them.
- vision.rs/ux.rs: optional local/cloud screenshot and novice judgments; deterministic workflow rules.

GPU coordination applies only to explicitly selected local inference. One selected model at a
time is coordinated within/across Kritic processes; unrelated applications are outside that
coordination. If the configured wait expires, contention is reported. No global VRAM guarantee.
The prior measured 12 GB spillover explains the sequential design; it is not an 8 GB benchmark.

Run reports/logs/models default under KRITIC_HOME. They can contain private source/output: sanitize
before sharing. Contract reports are evidence ledgers, not cryptographic execution attestations.
External commands execute with the user's privileges, not inside a security sandbox.

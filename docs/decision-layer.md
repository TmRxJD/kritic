# Optional decision helpers — test release

These helpers are disabled by default (`capabilities.context=false`). They are advisory ways
to rank files, trim context or choose between explicit options. They do not write code, replace
the calling agent's exploration, or prove a proposed change is correct. No token/cost saving is
promised without a measured comparison on the actual workflow.

`locate` ranks repository candidates using keywords and optionally typed model judgments.
If inference is unavailable it can return keyword ranking with a coverage note. Its checklist
is a suggestion about task scope, not acceptance evidence or a work contract.

`decide` returns a typed choice/yes-no assessment and escalation hint. Independently validate
the decision before acting. Cloud confidence is self-reported; thresholds are heuristics for
escalation, not measured reliability. `route`/`context_trim` are optional suggestions and never
override the user's instructions or model selection.

Hermes context integration is explicitly selected. Read [Hermes](hermes.md) before enabling it;
retain recoverable original material and verify relevance decisions. [Benchmarks](benchmarks.md)
and bench/locate scripts are measurement tools, not guarantees of this release's performance.

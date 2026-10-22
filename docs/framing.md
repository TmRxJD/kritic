# Debugging-framing review — experimental review

`framing_critic` checks whether a debugging case preserves the evidence and causal
relationships needed to choose the next investigation. It needs no model or GPU. It does
not prove a root cause, execute a proposed experiment, or turn a well-formed case into
task acceptance. Independently inspect both the findings and the underlying artifacts.

## Enable and run

Use v0.5.0-test.5 or later with matching configuration and skill documentation.
The built-in and minimal-profile default is `capabilities.framing=false`. To opt in:

```toml
[capabilities]
framing = true
```

`KRITIC_CAP_FRAMING=true` is the environment equivalent. Restart the MCP server/client
and confirm tool discovery. No model, package-manager installation or API key is required.

```sh
kritic framing --schema
kritic framing --base-dir .
kritic framing --base-dir . --case .kritic/debug-case.json
# From the Kritic checkout, inspect the synthetic example:
kritic framing --base-dir . --case examples/debug-case.json
```

The default case is `.kritic/debug-case.json`. The MCP request is:

```json
{"base_dir":".","case_path":".kritic/debug-case.json"}
```

Use the emitted schema and [example case](../examples/debug-case.json) to construct the
case. The example is synthetic evidence, not a verified diagnosis of a real project.
Replace its artifacts, hashes and relationships with the actual current investigation.
The tool reads the explicitly selected local case and its bounded evidence corpus. It
does not index a workspace, execute commands, acquire GPU resources or make network calls.

The case is capped at 1 MiB, with at most 64 evidence files, 1 MiB per file / 8 MiB total.
The schema also bounds stages, links, experiments and other collections. Unsafe paths,
unknown fields and dangling references are errors. Missing files or budget-limited evidence
remain explicit gaps, never silently verified records.

Statuses are `consistent`, `incomplete`, `contradicted` and `stale`. `consistent` means only
that the supplied record satisfied these checks; it is not root-cause proof or approval.
CLI exit 0 means a report was produced, including reports with gaps or contradictions.
Malformed input, I/O failures and disabled capability return an error. Never use the exit
code alone as an acceptance gate.

## A causal case, not a checklist

State the observed failure and the desired behavior separately. Identify the relevant
observations, candidate explanations, attempted interventions and the next experiment.
Connect those records by explicit identifiers, preserving evidence that contradicts a
preferred explanation. Bind artifact observations to their actual content and context;
an earlier observation must not silently become evidence about a changed artifact.

Observation JSON contains `context` with matching `revision`, `run_id` and `input_digest`,
plus `data` checked through explicit JSON-pointer assertions. These are caller declarations,
not execution attestation or verification against current Git HEAD. Original-failure evidence
must assert actual `/data/symptom_persists: true`. Trace links bind actual `/data/from` and
`/data/to` stage identifiers; reusing a generic passing observation cannot manufacture a trace.
Configuration declares its owner stage, consumers and ordered low-to-high-precedence sources,
with effective values checked against the observed content.

Schema-file hashes show file presence, not validation. Applicable schemas need an observation
bound to actual `/data/stage`, `/data/schema` and `/data/enforced: true`. These checked claims
still do not prove a validator ran. For a stage without a schema boundary, provide a concrete
`schema_not_applicable` reason instead of inventing a schema merely to satisfy a field.

A single-cause explanation can be a good working frame when it accounts for the observed
failure, relevant counterexamples and the results of attempted fixes. Its presence in a
valid case is not proof that it is the true cause. The calling agent must test the causal
prediction and inspect results independently.

Scattering new flags, timeouts, retries or fallback settings across unrelated modules can
hide the mechanism instead of explaining it. Multiple configuration changes are not
automatically wrong; show which shared mechanism makes them necessary. Repeated failed
attempts without a changed hypothesis or discriminating experiment are a reason to
reframe, not to keep repeating the same intervention with different wording.

The deterministic checks expose stale hashes/context, broken evidence references,
refuted explanations, repeated failed attempts and other explicitly represented gaps.
They cannot detect every missing observation, assess every explanation's truth, or infer
a complete causal graph from prose. Empty/generic records and satisfied field counts
must never substitute for actual evidence. Malformed cases and unsafe paths are errors,
not clean reviews.

## Independent review packet

Use the full saved review packet, including the supplied case, evidence provenance,
relationships, findings and limitations, when asking another reviewer for an independent
opinion. Have that reviewer inspect the artifacts and consider alternative mechanisms;
do not ask it merely to endorse the first critic's conclusion. Review packets may contain
private paths, observations and source excerpts. Keep them private or sanitize deliberately
before sharing. The tool does not send that packet to another model or service.

MCP returns at most ten findings and ten evidence rows, with bounded detail/path text and
the original total counts. `full_result` points to the full saved report and review packet;
read it before giving an independent review or treating the visible list as complete. The
CLI prints the full report. Artifact paths and hashes let an authorized local reviewer inspect
source/schema files; the packet's checked observation contents do not replace that inspection.

## Mid-debugging exception

Ordinary Kritic task review stays at the end of a completed change. Framing is a narrow
exception: when debugging stalls, observations conflict, or repeated fixes fail, an enabled
`framing_critic` may review the current causal case before the next attempt. This is not
permission for per-file/per-edit critic loops, automatic hooks, command execution or model
downloads. A useful result identifies what observation or experiment could distinguish
explanations; it does not authorize a speculative refactor or claim that the bug is fixed.

After investigating, rerun the project's real validators and affected behavior. A framing
review never replaces the final work-contract, artifact or user-visible acceptance checks.

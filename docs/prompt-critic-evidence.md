# Prompt intake test evidence — 2026-11-19

Source-build prototype on Windows. These are synthetic task fixtures with **real
authenticated native Codex inference**, not claims about production task success.
The account-selected default model was used; no model identity or entitlement is
inferred from an available-model catalog. The official CLI managed its own login.

The final live fixture repeated this hypothetical request across three paragraphs:
change `parse::<u32>()` to `parse::<u64>()` in `src/main.rs`, then run `cargo test`.
No source change or test command was executed by the reviewer.

| Check | Observed result |
|---|---|
| Input | 597 UTF-8 bytes, 98 whitespace-delimited words |
| Final brief | 100 bytes; source code expressions, file path and test command retained |
| Original | Exact input bytes retained in report, with SHA-256 |
| Proposal | `suggestion`, quote mappings present, no literal-guard rejection |
| Authority | `replacement_allowed=false`, semantic equivalence explicitly unproven |
| Native transport | Fresh ephemeral read-only CLI turn completed with configured MCP/execution/web/apps/plugins/subagents disabled |

The brief was: “I would like you to change `parse::<u32>()` to `parse::<u64>()` in
src/main.rs and run `cargo test`.” This is a task brief, not a replacement instruction.
Native hooks retain the original as well, so this is **not net input-token savings**.
An earlier run generated an 82-byte brief; output varies between model calls.

Earlier integration attempts exposed real protocol errors: quoted MCP override
names created different entries, and inventory with apps/plugins enabled included
virtual bridge servers that could not be overridden as normal transports. The
implementation now inventories with the same disabled features and targets enabled
simple server names only. Unsupported names fail closed before inference. An
ambiguous fixture's malformed clarify proposal was rejected; the model instruction
now spells out the exact keep/clarify schema contract rather than weakening validation.

Offline coverage: 11 core/native-event behavioral tests, three Hermes adapter
tests, all-target compilation and core all-target Clippy. Negative cases include
missing paths/versions/prohibitions, invented commands, invalid mappings, fenced
code loss, filename/URL/decimal/line-wrap guard bypasses, malformed completion and
tool-call events. A positive case preserves a protected paragraph while shortening
repetition elsewhere. Independent review found the punctuation guard defect and
reported no further scoped authority/fail-open findings after its fix.

Release verification adds 29 compiled CLI/MCP checks covering local and all four
API transport envelopes, credential header placement with synthetic keys, malformed
responses, truncation, tool-call rejection, native hook augmentation and recursion.
These use loopback inference fixtures, not paid external API-model requests.

The actual installed Hermes `PluginManager`/`PluginContext` dispatcher also passed
13 checks through the real prompt adapter and compiled CLI child: supplementary
context, unchanged user message, duplicate/recursion suppression and HTTP 503
fail-open behavior. Inference was a loopback fixture. No global plugins were changed.

Seven further compiled CLI/MCP checks exercised session/handoff evidence and actual
Git ownership transactions, including dirty-work preservation, source fencing,
successor resume and stale-fixture rejection. Session identities were synthetic;
this is not a claim of an automatic real-session controller.

PRs #40 and #41 passed Windows linking/runtime tests and security plus Ubuntu,
macOS and browser CI. Local full linking was avoided because measured commit reserve
was only 6.82 GiB. A live small-Qwen GPU probe was stopped by its memory guard before
the six-GiB reserve was crossed; model availability depends on real host resources.
These results do not prove arbitrary intent preservation, production usefulness,
every cloud account's compatibility or complete harness behavior.

See [setup, provider limits and pipeline boundaries](prompt-pipeline.md).

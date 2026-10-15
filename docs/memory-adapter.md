# Optional review history and jcode-format export

Test.4 and later can retain explicitly selected review snapshots for later comparison.
It is disabled by default,
including in the minimal profile; it needs Git and writable local storage, but no model,
embeddings, GPU, cloud API, extra runtime or background indexer.

## Enable and record

In the server's `kritic.toml`, merge:

```toml
[capabilities]
history = true
```

Alternatively set `KRITIC_CAP_HISTORY=true`. Restart MCP and check tool discovery. Explicit
`tools.disabled` exclusions still take precedence. Storage is `runs_dir/history`, with a separate
directory for each canonical Git-root identity; moving a clone can give it a different identity.
Use an appropriate persistent private location for [configuration and reports](configuration.md).

First run the actual critic/validators. Save its complete JSON report, then explicitly record
that report and the exact relative input files it assessed. A scope description is not a glob:
no workspace discovery or automatic corpus indexing happens here.

```sh
kritic history --base-dir . record --report /absolute/path/review.json --inputs src/file.rs tests/file.rs --context 'tool=artifact_validate@source-revision;policy=policy-hash;commands=cargo-test-version;model=off'
kritic history --base-dir . list --offset 0
kritic history --base-dir . compare --left LEFT_RECORD_ID --right RIGHT_RECORD_ID
kritic history --base-dir . export-jcode
```

On Windows, use an absolute report path such as `E:/AI/reports/review.json`. Replace the context
example with concrete tool/version, model, command/configuration and policy identities. The
context is caller-declared and hashed; Kritic does not attest that the declared commands or
versions actually ran. Include meaningful changes in the context instead of reusing a convenient
label merely to make records comparable. Only supported Kritic report identifiers are accepted.

The equivalent MCP tool is `review_history` with these tagged actions:

```json
{"action":"record","base_dir":"/absolute/repo","report":"/absolute/review.json","inputs":["src/file.rs"],"context":"tool=artifact_validate@revision;policy=hash;commands=exact-command;model=off"}
```

```json
{"action":"list","base_dir":"/absolute/repo","offset":0}
```

```json
{"action":"compare","base_dir":"/absolute/repo","left":"LEFT_RECORD_ID","right":"RIGHT_RECORD_ID"}
```

```json
{"action":"export_jcode","base_dir":"/absolute/repo"}
```

## What records establish

Records contain the saved report, canonical-JSON report hash, explicit input-path hashes, declared context hash,
Git revision when available and repository identity. Input file contents are hashed, not copied
as a separate source corpus. The report itself can still contain source snippets, secrets,
machine paths or execution output: inspect it before retaining or sharing it.

Every summary is `historical_unverified`. There is no approval verdict, cached acceptance,
automatic reuse of an earlier pass or claim that current inputs were validated. Hashes identify
recorded bytes; they do not establish who generated the report, execution authenticity, semantic
correctness or resistance to a local actor rewriting data. Associating a report with input/context
hashes does not prove the report was executed over those inputs: an old report can be supplied
with newer files. There is no automatic freshness verification. Rerun current checks for current work.

`list` returns 25 metadata entries per page, `total`, `offset` and `next_offset`. `compare` only
compares records from the same repository with matching tool and declared context identities.
It reports changed input hashes, revision changes and report-field differences represented by
hashes. These are observations, not a quality trend or proof that a defect was fixed. At most
100 report differences are returned; inspect `differences_truncated` rather than assuming the
displayed set is complete. Historical records do not become fresh merely because hashes match.

## Limits and retention

| Bound | Limit |
| --- | ---: |
| Saved JSON report | 8 MiB |
| Explicit input files | 1–512 |
| Each input / combined input bytes | 2 MiB / 32 MiB |
| Declared context | 12–65,536 bytes |
| Stored record | 12 MiB |
| Per-repository records / stored record bytes | 512 / 64 MiB |

Inputs must be explicit repository-relative regular files without traversal; duplicate paths,
directly symlinked/nonregular files and paths resolving outside the repository are rejected. Exceeding a budget fails;
Kritic does not silently drop records or delete history. Export files and ordinary run reports
are separate from the history-store budget and also need deliberate retention management.

Protect `runs_dir/history` as private project data. Archive or remove only the chosen project's
history directory after verifying its resolved location and that no writer is active. There is
no automatic purge and no delete command in this interface. Do not delete the entire run root
or another project's records to clear a limit. A stale writer lock requires explicit inspection
and removal only after confirming no writer owns it.

## jcode-format interchange

`export_jcode` writes a separate JSON file and returns `export_path`, entry count and
`live_jcode_modified=false`. The format follows the
[pinned upstream memory types](https://github.com/1jehuang/jcode/blob/ac2a0b09848a9ecc09942b8faf196b3af69a65bb/crates/jcode-memory-types/src/lib.rs)
at revision `ac2a0b09848a9ecc09942b8faf196b3af69a65bb`. Exported entries have low trust and
confidence `0.0`; their sanitized content includes the tool and report/context/input hashes.
Source text, report contents, raw context and local path strings are omitted from the export.
Hashes and timestamps remain metadata that may be sensitive or correlatable.

Validation on 2026-10-15 generated a nonempty export fixture from Kritic's history test, then
deserialized it with the unmodified upstream `jcode-memory-types` crate at the pinned revision.
The actual `MemoryStore` accepted the custom category, low trust and zero confidence, and a
typed JSON serialization/deserialization round-trip passed. This checks that fixture's format;
it does not establish compatibility with every future jcode version or live import behavior.
Set `KRITIC_HISTORY_EXPORT_FIXTURE` to an output file when running `cargo test --test history`
to generate a fixture for an independent compatibility check.

This is a format export, not a verified live jcode integration. It neither merges into a running
jcode store nor installs a jcode harness, and it does not claim a live round-trip passed. Review
and verify compatibility before any separate import. No model accuracy, token reduction, RAM
savings or long-term memory benefit has been established by this adapter.

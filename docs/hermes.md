# Hermes Agent

[Hermes Agent](https://github.com/NousResearch/hermes-agent) (Nous Research) is a general agent
harness that works with any model API. kritic supports it in two ways:

1. **MCP server**: every kritic tool is available to Hermes, exactly as in Claude Code.
2. **Context engine**: when Hermes calls a model through an API, the conversation lives in
   Hermes, not in a subscription client. kritic can manage that context: archive instead of
   summarise, with a recall tool.

```bash
kritic register --hermes --hermes-context
```

`kritic setup` does this for you when it finds Hermes. Start a new Hermes session afterwards.

## What registration changes

| Where | What |
|---|---|
| `mcp_servers.kritic` in Hermes' `config.yaml` | the kritic MCP server |
| `context.engine: kritic` in `config.yaml` | kritic manages the conversation (only with `--hermes-context`) |
| `plugins/kritic/` in the Hermes home | the context-engine plugin and, when `output.style = "concise"`, a one-paragraph reply rule added to every turn |

Hermes keeps its data in `%LOCALAPPDATA%\hermes` on Windows and `~/.hermes` elsewhere
(`HERMES_HOME` overrides both). kritic backs up `config.yaml` before editing and
`kritic uninstall` restores the previous engine (`compressor`) and removes the plugin.

Hermes leaves user plugins disabled until asked, so `kritic register` runs
`hermes plugins enable kritic` when the `hermes` command is on your PATH (if not, run it once
yourself).

## How the context engine works

### Failure and ownership policy in the audited local stack

The local Hermes integration requires the engine named by `context.engine`. A load,
clone, registration or configuration failure stops initialization; it does not select
the built-in summarizing engine. `context.engine: compressor` explicitly selects that
engine. Duplicate context-engine registrations are fatal even if discovery catches
the registration exception. These are local maintained-runtime changes, not a claim
about every upstream Hermes version.

An explicitly supplied `KRITIC_EXE` must name an existing absolute file. An invalid
override or recorded executable path raises an error instead of trying another
binary. PATH discovery applies only when neither is supplied. Missing configuration
uses the documented defaults; malformed, unknown or incorrectly typed
`kritic_context` settings raise errors. This separates deliberate defaults from
unexpected fallback.

The end-of-task model critic remains advisory. Offline or timed-out review is
unscored; independent test failures remain failures. Engine loading and successful
tests do not prove context savings or model quality.

Hermes' default compressor asks a model to summarise the middle of a long conversation. A summary
is written by a model, so it can lose the one line you needed and it costs a model call. kritic
does the opposite:

- Whole middle messages are **archived** to `<kritic home>/context/<session>/archive.jsonl`,
  keyed by a content hash (`kr-` plus 12 hex characters), and replaced by a one-line stub that
  names the id.
- Nothing is rewritten. What the model sees was written by a person or a tool, never by another
  model, and the archive is lossless.
- The agent gets a `kritic_recall` tool: `kritic_recall(id)` returns the full message,
  `kritic_recall(id, query)` returns only the matching lines.
- **Order of removal:** exact duplicates first, then large tool results (over `big_chars`), then
  the messages Kev judges least relevant to the current task. If Kev is unavailable it falls back
  to oldest first. The first `protect_first_n` and last `protect_last_n` messages, message
  structure, `tool_calls` and images are never touched.
- If the archive cannot be written, nothing is archived. The plugin fails open: on any error the
  conversation passes through unchanged and the error is logged.

Settings live in a `kritic_context:` section of Hermes' `config.yaml` (all optional):

```yaml
kritic_context:
  threshold: 0.5        # compact at this share of the model's context window
  target: 0.35          # ...down to this share
  protect_first_n: 3    # non-system head messages kept verbatim
  protect_last_n: 6     # tail messages kept verbatim
  big_chars: 8000       # tool results above this are archived when pruning
  use_kev: true         # rank by relevance with Kev; false = oldest first
  timeout_s: 90         # one kritic call
```

## Measuring it

The plugin logs each model response's usage to `<kritic home>/logs/hermes-usage.jsonl`.
`token_metrics(source="hermes")` reports prompt tokens, output tokens and compactions for a Hermes
session, the same way it reports Claude Code sessions.

## Versions

Tested with hermes-agent 0.19.0 on Windows. That version lacks some optional context-engine hooks
(`prune_tool_results_only`, `select_context`, `on_turn_complete`, `clone_for_agent`); the plugin
implements what Hermes calls and logs usage from `update_from_response`, so it works on both
older and newer releases. The plugin has its own tests:
`uv run --with hermes-agent --with pytest pytest integrations/hermes/tests`.

## Automating with Hermes

Context archives use `context/v2/session-<SHA-256>/archive.jsonl`, hashing the exact session ID
so case, punctuation and long names cannot merge sessions. New record IDs use full SHA-256.
Original JSON content is retained alongside searchable text. Default recall returns text only,
without duplicating archived JSON. Explicit `format="original"` returns exact JSON in
`original_content`, with an empty text field. It reports an insufficient-budget error instead
of truncating JSON. Text-part boundaries survive original-format recall, and multimodal messages
remain protected. Retention is lossless; a bounded prompt still contains only selected content.

`max_chars` is a UTF-8 content byte budget (default 20,000); original format counts serialized
JSON bytes. `max_output_bytes` bounds the complete compact JSON response, including metadata
(default 12,000; accepted range 256–4,194,304). Text recall may shorten or omit records to fit;
`truncated_chars` reports omitted UTF-8 content bytes. For exact retrieval, request one archive
ID with original format and enough room in both budgets. Archive storage is never truncated.

Archive readers and writers coordinate with OS file locks. Writers validate existing records,
deduplicate under the lock and sync data before returning stubs. A read, corruption or write
failure is reported; failed compression preserves the original messages. This supports cooperating
processes on a local filesystem, not arbitrary external writers or distributed storage.

Legacy short IDs remain readable from the old directory for exact safe session IDs (nonempty,
at most 96 ASCII letters, digits, underscores or hyphens). Ambiguous legacy names that were
sanitized or truncated require explicit archive migration after verifying their session; they
are never automatically merged. Keep legacy archives while old transcript stubs reference them.

Hermes can run tasks on a schedule and from messaging platforms. kritic is useful there because
an unattended agent has nobody to notice a bad change: give the scheduled job the
[kritic skill](claude-code.md#the-skill)'s instructions ("run `critic_score` and
`artifact_validate` before reporting done") and its result carries an independent verdict.

### Codex task routing

The MCP `route` tool now defaults to the typed `route_codex` spec. Its
`coding_model` is one of `gpt-6.1-sol`, `gpt-6-astra`, `gpt-6-luna`.
Unavailable, unknown or low-confidence answers fall back to Sol; this is
uncalibrated routing advice, not an execution or correctness gate. Set
`legacy_routing=true` to retain the earlier non-Codex tier vocabulary.
The old Rust `tools::route` API remains available for legacy callers.

On this Windows setup, Hermes uses the Rust MCP bridge to normalize equivalent
Boolean JSON Schemas for MCP 2.0, and the local laya autostart wrapper forces CPU
inference while GPU training continues. No local model generates source code.

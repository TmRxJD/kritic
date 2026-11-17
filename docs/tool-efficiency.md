# Tool call efficiency

Kritic can combine a common Git/CI read workflow into one MCP call. Enable it in Kritic's own configuration:

```toml
[capabilities]
git_ci = true
```

Reconnect your MCP client after changing capabilities. The workflow uses the same interface from Codex, Claude, and other MCP clients. Fetch short workflow descriptions with `workflow_discover`. Request `{"name":"git_ci_snapshot"}` for its exact argument schema, or combine search and schema lookup with `{"query":"pull request","include_schema":true}`. If you already know the inputs, call `workflow_run` directly:

```json
{
  "name": "git_ci_snapshot",
  "arguments": {
    "cwd": "/path/to/checkout",
    "pull_request": {"repo":"OWNER/REPO","number":36}
  }
}
```

Omit `pull_request` for a local-only snapshot. The workflow uses local Git for checkout information and the signed-in GitHub CLI only if you specify a pull request. CI output is untrusted evidence. Read the linked GitHub Actions job when the excerpt does not contain the information you need.

The local summary counts Git porcelain records, explicitly reports that it excludes submodule changes, and samples up to eight paths with their status. Use a trusted checkout. Kritic disables configured external content filters for this snapshot, which can change the reported dirty state for filtered files. CI counts describe the check rollup returned by GitHub. Kritic attempts one excerpt from the Actions job linked by the first failed or cancelled check when that URL points to the same repository. The reported SHA is the PR head returned by GitHub. The workflow does not independently verify the linked job's head. Unavailable or truncated evidence is reported explicitly, and a failed PR lookup still returns the local snapshot.

Use small, named workflows when the operations and output are predictable. Return the evidence needed to continue and a reference for inspecting the full result. The existing router sees the outer MCP call. Inside Kritic, the capability and workflow registry limit the runner to the named read-only operation. The host separately controls which schemas enter model context.

## Fresh-session benchmark

The fixture presents 24 unrelated tool schemas alongside a synthetic PR inspection. The direct workflow gets the head, checks, and log in three calls. The batched fixture returns the same head, failed check, and exact error in one call. Both paths include the same task prompt. Host deferral is enabled only for the deferred and batched cases. Runs start fresh with the fixture as the only MCP server.

Two runs of the first three modes on Claude Code 2.1.286 with Claude Sonnet 5.5, in opposite order, returned the same correct evidence and input counts. The fourth row shows a separate run with client deferral disabled:

| Mode | MCP workflow calls | Tool searches | Model turns | Total input tokens including cache | Wall time |
| --- | ---: | ---: | ---: | ---: | ---: |
| Direct | 3 | 0 | 4 | 69,672 | 5.56–5.66 s |
| Deferred direct | 3 | 1 | 5 | 25,094 | 6.44–6.91 s |
| Deferred batch | 1 | 1 | 3 | 12,332 | 4.59–4.88 s |
| Portable discovery + batch, deferral off | 2 (lookup and run) | 0 | 3 | 49,680 | 4.95 s |

The deferred direct run reduced cumulative input by 64%. The deferred batch reduced it by 82% against direct and 51% against deferred direct. A separate portable fixture run reduced cumulative input by 29% even with the unrelated schemas still loaded. The portable run uses Kritic's discovery and execution pattern over fixtures. These counts sum uncached, cache creation, and cache read tokens across all model turns. Billing treats these counters differently. The fixture intentionally carries redundant passing checks and compile messages. Its savings measure the mechanism under controlled conditions, and production savings depend on the host, enabled catalog, and task.

Run the fixture with a signed-in Claude CLI and an existing scratch directory. Each invocation runs one fresh model session per selected variant and writes its response and usage summary to the selected output directory. Use `--mode portable` to test only client-independent discovery and batching.

```sh
python bench/tool_efficiency.py --scratch /path/to/scratch --out /path/to/report
python bench/tool_efficiency.py --scratch /path/to/scratch --out /path/to/reverse-report --reverse
```

Use [the schema budget benchmark](schema-budget.md) to measure the advertised MCP surface of a real Kritic binary separately. For a production replay compare successful evidence, retries, model turns, context usage, and latency on the same task before and after adopting a workflow.

In a live replay against [Kritic PR #36](https://github.com/TmRxJD/kritic/pull/36), the built server matched the head, five successful checks, and no failed check reported by the direct commands. The snapshot was 534 serialized bytes versus 2,597 bytes from direct Git and GitHub CLI responses, excluding discovery and MCP envelopes. Enabling both workflow tools adds about 1.1 KB of serialized definitions to `tools/list`. A fresh Claude session with native deferral disabled retrieved the schema and ran the live workflow in two calls. The failed-log path was exercised with fixtures.

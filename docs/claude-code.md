# Using kritic with Claude Code

kritic is an ordinary stdio MCP server, so any MCP client can call it. The files in
[`examples/claude/`](../examples/claude) make Claude Code use it the intended way: once, at the
end of a task, as a second opinion.

## Setup

```bash
claude mcp add --scope user kritic -- /absolute/path/to/.kritic/bin/kritic
cp -r examples/claude/skills/kritic ~/.claude/skills/
cp examples/claude/agents/ux-critic.md ~/.claude/agents/
cat examples/claude/CLAUDE.md >> ~/.claude/CLAUDE.md
```

- **The skill** (`skills/kritic/SKILL.md`) is the workflow: which tools to call for which kind of
  change, how to read a rejection, the 2-revision cap, and the one-line report.
- **The CLAUDE.md rule** makes Claude load the skill when a task that changed files is finished,
  and never mid-edit.
- **The `ux-critic` agent** turns a `ux_critic` report into a designer's verdict (`restructure`,
  `revise` or `acceptable`) with layout sketches. It reads screenshots only, never source.

## Principles the integration keeps

- **End of task only.** No hooks, no per-edit runs, no blocking. A second opinion given after
  every keystroke costs time and context and trains the agent to chase a score.
- **Advisory.** kritic never overrides the user, the project's tests, or the agent's instructions.
- **Two revisions at most.** `revise_allowed` turns false after `review.max_revisions`. The agent
  then lists what is left instead of looping.
- **Never fabricate.** When Kev is offline, the report says "critic offline" with the error and
  the deterministic results, never a made-up score.
- **Flags are locations.** A Kev flag says where to look. The agent reads that code and fixes only
  a problem it can name, or reports the flag as a false positive. The [benchmark](benchmarks.md)
  shows why this matters: Kev's gates also flag some correct changes.

## Waiting without polling

Long tools (UI crawls, desktop runs, scanners) stream progress on a WebSocket. Call `status()`
once for `ws_url`, then subscribe with Claude Code's Monitor tool
(`Monitor({ws: {url: ws_url}})`). Each connection gets a `snapshot` of ready states, then:

- `ready`: laya `starting`/`up`/`stopped`/`failed`, vision `loading`/`loaded`/`unloaded`, gpu
  `held`/`free` with the owning tool. Only state changes are sent.
- `tool`: `start` and `end`, with approval and duration.
- `progress`: critic chunks (`done`/`total`), each `ui_test`/`desktop_test` page and interaction,
  plugin progress lines, and app events relayed from a Unity app.

If another session's server already holds port 8790, a free port is used; `status()` reports the
actual URL, and `logs/ws/<pid>.url` lists it too.

## Context use

- Tool responses are compact: large reports go to `full_result` files, and the response keeps only
  problems and top items.
- `token_metrics()` reads the session transcript and reports:
  - per session and per task (each human prompt): input, cache read/write, output and thinking tokens;
  - processed input tokens;
  - context size now and at peak, as a % of the limit, and growth per call;
  - compactions, cache hit rate and output tokens/s;
  - local model usage.
- An MCP server cannot remove anything from the agent's context; only the client's compaction can.

**Don't set Claude Code's `autoCompactWindow` below the session's startup context plus headroom.**
Compaction fires at roughly the window minus a ~30k reserve. With many plugins and MCP servers, a
session can start at 60–70k tokens. A 100k window then compacts within seconds of the first prompt,
and keeps compacting about once a minute. Shrink the startup context instead, for example by
disabling unused tools with `[tools] disabled`.

## Other clients

Nothing in the server is Claude-specific except `token_metrics`, which reads Claude Code's
transcript format, and `route`, which names Claude model tiers. Disable both in other clients.

# Connect Codex, local Work, Cursor or another agent

Install the executable/config first using [the minimal quickstart](../README.md#quickstart-install-the-minimum-then-choose-extras).
Then connect **two things**: the MCP server provides checks; the skill tells the agent how to
interpret and independently validate them. Your host's existing model does the reasoning;
leave Kritic's model backends off. No additional inference account is needed.

Current source builds also offer `repository_critic` when `capabilities.repository=true`.
The minimal profile disables it; the published `v0.5.0-test.3` binary predates it. Client/MCP
registration does not enforce commits: configure repository rules and explicitly
[install the Git hook](repository-policy.md#commit-enforcement). Keep semantic scope review in
the calling agent, including with models off.

The optional source-only `review_history` tool is separately gated by `capabilities.history=false`
by default. Enabling it exposes explicit record/list/compare/export actions; it does not capture
your session, index the workspace or install a jcode harness. [Usage and privacy](memory-adapter.md).

## Codex desktop, CLI, IDE and local Work

Merge [examples/codex/config.toml](../examples/codex/config.toml) into your user
`~/.codex/config.toml` or trusted project's `.codex/config.toml`. Replace both paths with the
extracted package's absolute directory; Unix builds use `bin/kritic` instead of `bin/kritic.exe`.
The example starts stdio `serve`, persists KRITIC_HOME and allows long scanner/test calls.
Do not overwrite other MCP entries. See [official MCP configuration](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).

Alternatively, in the desktop app open Settings → MCP servers → Add server, choose STDIO,
provide the binary path, argument `serve` and KRITIC_HOME, then restart the connection.

Copy the bundled `examples/claude/skills/kritic` folder into your project's `.agents/skills/kritic`.
Its content is host-neutral despite the historical source directory name. A user-level
`~/.agents/skills/kritic` installation applies across local projects. Do not install both copies
with the same name. [Official skill discovery](https://learn.chatgpt.com/docs/build-skills).

For repo-wide end-of-task behavior, merge the small block from
[examples/agents/AGENTS.md](../examples/agents/AGENTS.md) into your project's AGENTS.md.
This preserves the test-release requirement to verify critics and contribute reproducible fixes.

In a local Codex chat, invoke `$kritic`; in Work select the skill with `@` when available, or
ask the agent to follow the installed Kritic skill explicitly. Check connected tools before use.

**Work Cloud / web is a separate deployment.** A local executable/config is not a remote MCP
service, and a cloud container cannot use a path on your PC. Cloud chats require available
plugin/remote-tool connectivity; local-computer access does not guarantee that local MCP or
local skills propagate. Kritic currently ships stdio, not a hosted HTTP MCP endpoint. Do not
enter its progress WebSocket URL as an MCP URL or expose it through a tunnel. Use a local
Codex/Work task for this package, or install/build Kritic in the actual executor and verify tools
and skill discovery there. GUI checks require access to that executor's GUI.

## Cursor local IDE / Agent

Merge [examples/cursor/mcp.json](../examples/cursor/mcp.json) into project `.cursor/mcp.json`
or user `~/.cursor/mcp.json`. Replace both absolute paths. Keep other `mcpServers` entries;
restart Cursor and check Kritic in Customize → MCPs.
[Official Cursor MCP setup](https://prod.cursor.com/help/customization/mcp).

Copy the same bundled skill folder into project `.agents/skills/kritic` (shared with Codex)
or `.cursor/skills/kritic` (Cursor only), then invoke `/kritic` from Agent chat. Choose one
location to avoid duplicate discovery. For persistent project guidance, merge the supplied
AGENTS.md block into your existing project instructions.
[Official Cursor skill directories](https://prod.cursor.com/docs/skills).

Cursor Cloud Agents are separate executors: local binary paths and user skill folders do not
automatically give remote workers Kritic. Install the matching binary/tool dependencies on the
worker and use its supported MCP configuration, or a separately deployed remote service.
This release has no hosted Kritic endpoint. The agent can still use its own tools to follow the
skill's evidence discipline, but that does not prove Kritic MCP checks ran.

## Claude Code / Hermes

After selecting your capabilities, run `kritic register --claude` or `kritic register --hermes`
with the full binary path if it is not on PATH. These are the only built-in registration targets.
Codex and Cursor use the explicit configuration above; there are no `--codex` / `--cursor` flags.
Hermes context-engine registration is a separate opt-in (`--hermes-context`), not required for review.
For another stdio client, configure the same binary with `args=["serve"]` and KRITIC_HOME;
use its skill mechanism or merge the AGENTS.md instruction block.

## Prove the connection before a real task

Ask: “Use Kritic's TEST RELEASE skill. Inspect available tools and call status(start=false).
Review this small change with artifact_validate(use_model=false), using its absolute repository
path. Independently verify the results and report any skipped coverage. Do not enable models.”

Confirm the actual server reports the expected config, then supply a real changed file and a
meaningful project validator. `status` with no review is only a connection check. A minimal
profile intentionally hides browser, scanner, desktop and model tools; add dependencies and
enable groups before asking for those reviews. Use absolute base_dir/paths so the host's launch
directory cannot silently select a different repository.

Configurations are parsed and their stdio launch tested in release preparation. The complete
Cursor GUI workflow and every local/cloud Work mode have not been exercised on friends'
machines; treat those as field-test targets and report exact failures rather than assuming parity.

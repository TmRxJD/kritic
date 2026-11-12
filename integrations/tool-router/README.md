# Tool router (skills + MCP + plugins + shell, Claude Code + Codex)

Source of truth is `integrations/tool-router/` in the kritic repo; `deploy.py` copies it to `~/.claude/skill-router` (keeps an existing `tools.json`).

`tool-router.py` keeps the per-call baseline small and forces every task to declare which non-core tools it needs.
State/backups/usage live in `B:/AI/tool-router` (E: was full on 2026-11-11). Config: `tools.json` (packs, core MCP, protected MCP).
Run: `python -I C:/Users/jdion/.claude/skill-router/tool-router.py <cmd>`.

## Agent workflow
1. SessionStart context gives the session id. `catalog` (~250 tokens) lists packs; `catalog <pack>` lists items with descriptions; `catalog --full` (~2.3k tokens).
2. **Declare**: `plan <pack...> --session ID --task "..."`, `plan --preset NAME --session ID`, or `plan --none --session ID` (core only).
3. Re-run `plan` to extend when scope changes. Plans are **live** (checked on every tool call).
4. `enable|disable PACK|NAME [--host codex]` change what is installed (**next session**); use `tool:NAME` when a pack and tool share a name (e.g. `blender`).

## Enforcement (hooks, verified in Claude Code 2.1.286 and Codex 0.167 binary strings)
- Claude `~/.claude/settings.json` + Codex `~/.codex/hooks.json`: SessionStart (sync + context), UserPromptSubmit (nag), PreToolUse matcher `mcp__.*|Skill` (Codex: `mcp__.*`) returns `permissionDecision: deny` with the exact command to run.
- Built-in tools, core MCP (`terminal, visualize, Claude_Browser, ccd_*, scheduled-tasks, mcp-registry, ...`) and **protected** `kritic`/`kev-tools` are always allowed and can never be toggled.
- Hooks fail open on internal errors. `rtk hook claude` entry is preserved.
- New skills: any skill on disk not in `registry.json` and not in a pack is denied until `register NAME --pack P | --core` (also announced at SessionStart/UserPromptSubmit). `baseline` registered the current set. `unregistered` lists them. Skills installed by other tools are caught on the next hook run.
- Codex: hooks need trust approval in the Codex app the first time (hash in `config.toml [hooks.state]`); not done here.

## Presets (per repo)
`preset set NAME --packs a,b [--tools x] [--note T]` stores in `<repo>/.claude/tool-router.json`; `preset list`; `preset delete NAME`; `plan --preset NAME`. Global presets can go in `tools.json` under `"presets"`.

## Packs / project detection
Markers (`*.uproject`, `*.blend`), path hints, or forced via `enable PACK`. `sync` (SessionStart) links pack skills as junctions into `<repo>/.claude/skills` and re-adds library MCP of active packs at local scope. Removed MCP definitions are kept in `mcp-library.json` (seeded from `mcp-removed-20261111.json`). Codex MCP/plugins toggle with `enabled = false`; Codex skills move to `~/.codex/skills-disabled`. Nothing is deleted.

## Reversibility
Every mutation snapshots the files first: `backups` lists, `restore [ID]` restores (latest by default). `uninstall` removes the hooks. Old `skill-router.py` still works standalone.

## Kritic
Plugin `.kritic/plugins/tool-router.toml` (extension point: repo plugin, no core change). `plugin_list/plugin_run(base_dir="C:/Users/jdion/.claude/skill-router", name="tool-router", args={action: audit|catalog|plan, session, project, task, packs})`.
`audit` is deterministic (plan vs usage log: calls without plan = high, denied calls = info, unused planned packs = low, unregistered skills = medium) plus an advisory Kev question `fit` (are used tools relevant to the declared task). Advisory only, uncalibrated.

## Tests
`python test_tool_router.py` (30 checks on a temp project; fake `claude` CLI, temp Codex dir; touches no real config).

## Subsets
`plan unreal:ue-material*` allows only matching skills of a pack; `plan rea:list_*` only matching MCP tools of a server (also full-name globs `mcp__server__read_*`). `enable unreal:ue-material*` links only those skills into the project (MCP servers are enabled whole; tool-level limits are plan-only).

## Shell through kritic
`cli enforce on` makes every session started afterwards (they hold a marker, so their kritic has `cli_run`) deny Bash/PowerShell with the exact `mcp__kritic__cli_run` call to use. Running sessions are unaffected. Commands containing `tool-router.py` are always allowed (bootstrap). `cli bypass on` (or file `<state>/cli-bypass`, or env `TOOL_ROUTER_CLI_BYPASS=1`) lifts it. `cli_run`: `program`+`args`, `stdin`, `cwd`, `env`, `timeout_s`, or `script {language, code, args}`; `dry_run` previews; `bash -c`/`pwsh -Command`/`cmd /c` are rejected.

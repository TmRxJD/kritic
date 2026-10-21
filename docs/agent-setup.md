# Agent setup and recovery

Agents should repair ordinary, in-scope setup instead of relaying raw missing-file errors.
The full [agent setup reference](../examples/claude/skills/kritic/references/setup.md) ships with
the skill and covers every critic, project contracts, dependencies, optional models and recovery.
Use documentation from the same source/package version as the binary.

Current source adds read-only project preflight:

```json
{"base_dir":"<actual repository root>","start":false}
```

Pass this to `status`. Its optional `project_setup` section lists every tool's effective toggle,
default project manifest, presence state, required/optional status and corrective action. Calling
`status` without `base_dir` keeps the usual small response. No new MCP tool is required.

With the matching executable:

```sh
kritic doctor --base-dir <repository-root> --json
```

`checks` retains machine/dependency diagnostics; `project_setup` adds the same project guidance.
Preflight does not create files, install packages, start services, run commands, index code or
validate a manifest's contents. `present_unvalidated` is not ready, tested or approved. Unity,
vision, models and domain-specific tools should remain unused when irrelevant.

When contract loading fails, its report preserves the original error, identifies `base_dir` and
`contract_path`, and adds a structured `setup` recovery plan. The agent should derive a missing
contract from the project's existing rules and real validation commands, then reread it with
`run_commands=false`. Never create a generic all-passing contract or fabricate execution evidence.

These runtime additions require a matching new source build; older releases may omit preflight.
The reference explains the manual fallback using supported `status`, `doctor --json` and project
inspection. Binary/global configuration changes require reconnecting MCP; project files are
reread per critic call. Setup guidance does not confer execution or publication permission.

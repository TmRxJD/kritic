<!-- Paste into ~/.claude/CLAUDE.md (every project) or a repo's CLAUDE.md. -->

## End-of-task review (kritic, advisory)

kritic is a local MCP server: deterministic checkers (tests you name, parsers, scanners, linters,
Playwright, UI Automation, a resource sampler) produce findings; the Kev-4B typed-decision model only
triages and scores them; approval is computed in code. It never overrides the user or the tests.

- Run it ONLY when a task that changed code/config/UI files is finished. No mid-edit or per-file runs.
- Load the `kritic` skill and follow it: `critic_score(git_diff=true, ...)` + `artifact_validate(paths,
  commands=[the project's test/tsc/lint])`, plus the specialist tools the change calls for.
- `approval: false` → command and parser failures are real; Kev flags are locations: read the code and
  fix only issues you can name. At most 2 revisions, then list what's left.
- Never fabricate scores. `kev_ok: false` → say "critic offline (<error>)" and report the checks.
- Final message gets one line: `kritic: A9.1 C8.7 Q7.0 · sev none · tests pass · approved · 0 revisions`.

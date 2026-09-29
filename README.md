# kritic

**A local, advisory end-of-task review for coding agents.** When an agent is about to say "done",
kritic checks the change: real tools produce the findings, the
[Kev](https://github.com/jaredpalmer/kev) typed-decision model only scores and ranks them, and
approval is computed in code from thresholds that carry their calibration evidence.

It runs as an MCP server (Rust, stdio) on your machine, with no cloud calls for the review itself,
and is built for [Claude Code](https://claude.com/claude-code). Any MCP client works.

```text
kritic: A9.1 C8.7 Q7.0 · sev none · tests pass · UI 24 actions 0 breakages · approved · 0 revisions
```

## With vs without

14 realistic agent changes, each in a defective and a correct variant. *Without* = the project's
own tests, which the agent already runs. *With* = those tests plus the review kritic prescribes for
that kind of change ([method and per-case results](docs/benchmarks.md)):

| | Without kritic | With kritic |
|---|---|---|
| Defects caught | 1 / 14 | **14 / 14** |
| …by kritic's deterministic tools alone | | 11 / 14 |
| Correct changes rejected | 0 / 14 | 7 / 14 (5 by Kev, just past its thresholds) |
| Review time | | median 7.6 s (1–94 s) |
| Tokens returned to the agent | | ~600 per review |

What that means in practice:

- The deterministic layer caught hard-coded secrets, SQL and shell injection, vulnerable
  dependencies, broken config, merge markers, a broken UI and a 24-branch function. None of them
  broke a test.
- Kev caught what no checker looks for: a stub left `not implemented`, a comment telling AI
  reviewers to approve, and an off-by-one.
- Kev also flags correct code, so the workflow treats its flags as places to read, never as verdicts.

## How it decides

```
 facts (code)            scores (models)               verdict (code)
 ─────────────           ───────────────               ──────────────
 tests you name     ┐                                  deterministic failure → false
 parsers, markers   │    Kev-4B: typed questions       calibrated Kev gate   → false
 scanners, linters  ├──► over declared state,  ──────► budget exceeded       → false
 Playwright, UIA    │    probabilities only            anything unmeasured   → null
 process sampler    │    Qwen2.5-VL: layout scores     otherwise             → true
 repo plugins       ┘
```

- **Models never invent findings, and never remove one.** Kev triages, ranks and scores facts that
  code gathered. A model opinion cannot clear a failing test, a secret or a crash.
- **Every Kev question is a declared spec** with defined answers, linted, and allowed to fail
  approval only with dated calibration evidence ([decision specs](docs/decision-specs.md)).
  Calibration found Kev good at unfinished work, reviewer-directed text, completeness and security in
  diffs, and blind to subtle logic bugs. So `defect` never gates, and your tests stay the bug catcher.
- **Never fabricated.** A tool that can't run returns `approval: null` with the reason.
- **Advisory, end of task, at most two revisions.** No hooks, nothing blocks, and nothing overrides
  the user or the tests.

## Tools

| | |
|---|---|
| `critic_score` | Kev over a git diff or files: unfinished work, reviewer-directed text, security, completeness against the request |
| `artifact_validate` | parse checks (JSON/YAML/TOML/Python, merge markers, UTF-8) + **your test/tsc/lint commands** |
| `security_critic` | gitleaks, osv-scanner, semgrep, bandit, cargo-audit; Kev ranks findings |
| `code_quality_critic` | your linters + complexity, length, nesting, duplication |
| `ui_test` | Playwright crawl of a local web app: errors, dead controls, failed requests, persistence, axe |
| `visual_critic` | Qwen2.5-VL layout scores for a page or screenshot |
| `ux_critic` | workflow review: mode entries, shortest task paths vs budgets, discoverability, with a novice model |
| `desktop_test` | Windows apps through UI Automation; **Unity mode** with an app event socket, soak, frame timing |
| `unity_tests` | Unity Test Framework, headless |
| `resource_critic` | CPU, memory, handle growth, GPU against budgets |
| `hygiene_critic` | orphaned processes, held ports, leftover files |
| `plugin_list` / `plugin_run` | your repo's own evaluators in `.kritic/plugins/` |
| `status`, `spec_lint`, `token_metrics`, `route`, `context_trim` | health, spec checks, context accounting |

Full rules: [docs/tools.md](docs/tools.md).

## Quick start

```bash
git clone https://github.com/TmRxJD/kritic && cd kritic
scripts/install.sh --with-models        # binary, harness, config, laya + Kev-4B (~4.5 GB)
ollama pull qwen2.5vl:7b                # optional: visual_critic and the ux novice
claude mcp add --scope user kritic -- ~/.kritic/bin/kritic
cp -r examples/claude/skills/kritic ~/.claude/skills/ && cp examples/claude/agents/ux-critic.md ~/.claude/agents/
cat examples/claude/CLAUDE.md >> ~/.claude/CLAUDE.md
```

Requirements per feature, GPU notes and scanner installs: [getting started](docs/getting-started.md).
Every setting is in [`kritic.toml`](examples/kritic.toml) ([configuration](docs/configuration.md)).

## Docs

- [Getting started](docs/getting-started.md) · [Configuration](docs/configuration.md) · [Claude Code](docs/claude-code.md) · [Troubleshooting](docs/troubleshooting.md)
- [Tools](docs/tools.md) · [ux_critic](docs/ux-critic.md) · [Unity](docs/unity.md) · [Repo plugins](docs/plugins.md)
- [Decision specs and calibration](docs/decision-specs.md) · [Measurements](docs/measurements.md) · [Benchmarks](docs/benchmarks.md)
- [Architecture](docs/architecture.md) · [Extending](docs/extending.md) · [Changelog](CHANGELOG.md)

Examples: [config](examples/kritic.toml) · [Claude Code skill, agent, rule](examples/claude/) ·
[a repo plugin](examples/plugin-repo/) · [ux.toml for web and Unity](examples/ux/) · [.mcp.json](examples/mcp.json)

## Status and limits

- Developed and measured on Windows 11 with an RTX 5070 (12 GB). The server, web harness and model
  runtimes are cross-platform; `desktop_test` is Windows-only. Linux and macOS are untested.
- Kev's context is 2,048 tokens: code is scored in ~1,300-character chunks, at most 60 per call.
- Kev returns probabilities, not explanations: `flagged_chunks` says where, and the agent has to read the code.
- Kev and the vision model share the GPU one at a time, so a visual review makes the next Kev call
  pay a ~13 s restart.

## Credits and license

kritic is Apache-2.0. It runs [Kev](https://github.com/jaredpalmer/kev) (Apache-2.0,
[GGUF](https://huggingface.co/mys/kev-4b-GGUF)) on [laya/ggmlc](https://github.com/monatis/ggmlc),
Qwen2.5-VL through [Ollama](https://ollama.com), [Playwright](https://playwright.dev) with
[axe-core](https://github.com/dequelabs/axe-core), [pywinauto](https://github.com/pywinauto/pywinauto),
[psutil](https://github.com/giampaolo/psutil) and the scanners named above. Each keeps its own license.

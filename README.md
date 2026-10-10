# Kritic — test release

**Deterministic checks for coding agents, with optional local or cloud reviewers. No GPU required.**

This is an experimental test release. The main purpose of the skill is to test whether Kritic's
workflow and findings are useful and correct. Agents must validate critic claims independently,
reproduce defects in Kritic, fix them with regression coverage, and contribute pull requests.
An `approval` value is a result from declared checks, never permission to ship or proof a task is complete.

Kritic is a Rust MCP server. The skill tells your existing coding agent how to collect evidence,
check requirements and evaluate the findings. That agent can use the cloud model it already runs
on; Kritic needs no separate inference service or API key for this workflow.

## Start without a GPU

**Windows testers:** download the ZIP from the [0.5.0-test.1 release](https://github.com/TmRxJD/kritic/releases/tag/v0.5.0-test.1),
extract it into your chosen persistent directory, then open the extracted `kritic` folder:

```powershell
$env:KRITIC_HOME = (Resolve-Path .).Path
.\bin\kritic.exe setup --backend off --no-vision
.\bin\kritic.exe doctor
```

No Rust installation or API key is needed. See [runtime prerequisites and client registration](docs/getting-started.md).
Install the bundled TEST RELEASE skill and ask your agent to independently validate each finding.

**Source build:**

Install Rust 1.89+ to build from source. Run in Bash (Git Bash on Windows):

```bash
git clone https://github.com/TmRxJD/kritic.git
cd kritic
export KRITIC_HOME="${KRITIC_HOME:-$HOME/.kritic}"
scripts/install.sh
"$KRITIC_HOME/bin/kritic" setup --backend off --no-vision
```

Set `KRITIC_HOME` first if you want data somewhere other than `~/.kritic`; for example
`export KRITIC_HOME='E:/AI/kritic'` on Windows. If you do not set it, invoke `~/.kritic/bin/kritic`
(Windows: `kritic.exe`). Setup can install browser/scanner dependencies and register Claude Code
or Hermes. `--no-register`, `--no-harness` and `--no-scanners` suppress those steps. No models
are installed unless you explicitly select local review or pass `--vision`.

Any MCP client can start `kritic serve` over stdio. Install the
[test-release skill](examples/claude/skills/kritic/SKILL.md) in your client's skill directory.
Claude Code: `kritic register --claude`. Hermes: `kritic register --hermes`.
For clients with no skill support, use the [short instruction block](examples/claude/CLAUDE.md).

## Choose your capabilities

| Capability | Model/GPU needed? | Evidence and limits |
|---|---|---|
| Artifact validation | No | Parsing, conflict markers, requested tests/typecheck/lint; only tests that actually ran count |
| Work contracts | No | Declared acceptance checks, input hashes, stale/missing evidence, static enforcement wiring |
| Web and UX checks | No | Chrome/Edge interactions, errors, persistence, axe rules, declared workflow paths; needs browser/spec |
| Security and quality | No | Installed scanners, project linters and code metrics; missing tools mean incomplete coverage |
| Desktop and processes | No | Windows UI Automation, process budgets/hygiene; desktop needs Python/uv |
| Model code critic | Optional | Local Kev or your configured cloud API; coarse screening, never a substitute for tests |
| Screenshot/novice critic | Optional | Local Ollama or a cloud vision model; observations can be false |
| Context helpers / map domain | Optional | Disabled by default; opt in only when relevant |

`status` reports capability settings and exposed tools. `kritic doctor` checks dependencies;
`kritic doctor --online` explicitly makes a reviewer API/inference call. A configured key is not
proof that a provider works. Toggle groups in `[capabilities]` and individual tools in
`[tools].disabled`; restart your MCP client after changing configuration.

## Optional cloud or local review

Cloud transports: OpenAI, OpenAI-compatible chat endpoints (including gateways), Anthropic,
Gemini, and JEV. Supply your own exact model ID and credential environment variable.
Neither assistant subscriptions nor host-agent OAuth tokens are reused. Protocol/model support
must be tested with your provider; “OpenAI-compatible” is not a guarantee every model accepts
the same parameters. Your current coding agent can always review deterministic evidence itself.

```toml
[review]
backend = "cloud"          # default: off
[cloud]
provider = "openai-compatible"
url = "https://openrouter.ai/api/v1/chat/completions"
model = "your-provider/model-id"
key_env = "OPENROUTER_API_KEY"
max_output_tokens = 4096
```

Cloud judgments are advisory. Self-reported confidence does not inherit Kev's local calibration.
Code excerpts go to the selected endpoint; screenshots are sent only when cloud vision is
explicitly enabled. Default operation has no inference calls, although tools such as dependency
scanners, setup and browsers can access networks.

Local Kev: `kritic setup --backend local --no-vision`. Local vision: `kritic setup --vision`.
Local inference is optional: roughly 8 GB VRAM for Kev Q8, 12 GB recommended for sequential
Kev and vision. The 8 GB vision configuration remains a field-testing target, not a guarantee.
No GPU is required for deterministic checks or cloud review. 16 GB system RAM is a practical
recommendation; the tested project may require more. See [hardware and dependencies](docs/getting-started.md).

## Review a work contract

Copy [the contract example](examples/contract.toml) to `.kritic/contract.toml` and write real,
nonempty acceptance checks. AGENTS.md, task text and extra source documents remain prose for
your agent to interpret; they are never silently marked satisfied.

```bash
kritic contract --base-dir .
# After reviewing the declared shell commands:
kritic contract --base-dir . --run-commands
```

The first command audits without executing project commands. Exit codes: 0 declared required
checks verified, 1 violated, 2 unverified. Passing a weak assertion does not establish the whole
requirement. A CI text binding proves declared wiring is present, not that CI ran or branch
protection is active. [Contract format and assurance](docs/contracts.md).

## Testing and contributing

Read [the tester guide](docs/testing-release.md), [contributing](CONTRIBUTING.md), and
[security policy](SECURITY.md). Model-free validation:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
# Hermes tests use the real Hermes agent.context_engine ABC.
# Set PYTHONPATH to a Hermes checkout and KRITIC_EXE to this build's absolute binary path.
python -m pip install pytest
python -m pytest integrations/hermes/tests
python scripts/test_calibration_policy.py
```

Live model tests are opt in. Mock provider tests make no paid calls. Test fixtures deliberately
contain defective code and unfinished markers; they are not production implementation stubs.
Windows is the development platform; cross-platform support needs field confirmation. Do not
represent an unrun OS/provider/model combination as verified.

## Evidence from real use

- **GPU-free reviews in Towerium:** its [public validation record](https://github.com/TmRxJD/towerium/blob/778febf06ef81a606829997fea8be59a2b52cbe2/VALIDATION.md)
  records Kritic running 64 controller tests while Kev was off. On 2026-10-10 we independently
  reran the focused controller/aim suite independently and through Kritic: **78 passed, zero failures/skips**, with no model or build.
  This proves those assertions ran; it does not prove every critic verdict or the whole game correct.
- **Project-specific checks without a model:** Beat Agent's real `map-score` plugin rejected a
  synthetic map with notes sharing a cell and beat. This freshly verifies overlap detection only;
  it does not establish human mapping quality or the historical calibration scores.
- **Browser effects checked against real behavior:** a headless browser independently reproduced
  focus-only and scroll-only false positives. Both native and legacy testers now recognize them;
  inert controls remain unobserved. The native browser regression was actually executed.
- **Acceptance checks that resist false passes:** contract regressions reject stale input hashes,
  evidence changed by later commands, escaping paths and empty/optional-only acceptance. Missing
  scanner coverage and empty reviews remain unverified instead of producing perfect scores.

[Reproducers, provenance and limits](docs/field-evidence.md) distinguish recorded field results
from fresh independent verification. This is a test release: **canvas/audio/app-state effects can
remain unobserved**, and live providers, local GPU models and other platforms are not yet verified.
See [canvas observation issue #1](https://github.com/TmRxJD/kritic/issues/1); unobserved UI effects now yield `approval: null`, not a fabricated
broken-control finding.

## Documentation

[Getting started](docs/getting-started.md) · [Configuration](docs/configuration.md) ·
[Cloud reviewers](docs/cloud.md) · [Work contracts](docs/contracts.md) ·
[Tool behavior](docs/tools.md) · [Architecture](docs/architecture.md) ·
[Security audit](docs/security-audit.md) ·
[Test release](docs/testing-release.md) · [Historical measurements](docs/measurements.md)

Apache-2.0. Optional model runtimes, scanners, axe-core and other dependencies retain their own
licenses. Historical benchmark results describe the recorded configuration, not this release's
accuracy on arbitrary projects.

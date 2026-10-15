# Kritic — test release

**Deterministic checks for coding agents, with optional local or cloud reviewers. No GPU required.**

This is an experimental test release. The main purpose of the skill is to test whether Kritic's
workflow and findings are useful and correct. Agents must validate critic claims independently,
reproduce defects in Kritic, fix them with regression coverage, and contribute pull requests.
An `approval` value is a result from declared checks, never permission to ship or proof a task is complete.

Kritic is a Rust MCP server. The skill tells your existing coding agent how to collect evidence,
check requirements and evaluate the findings. That agent can use the cloud model it already runs
on; Kritic needs no separate inference service or API key for this workflow.

**Work-contract critic included:** `contract_critic` audits declared requirements, evidence
freshness, weak acceptance checks and enforcement wiring. It is enabled in the minimal profile
and needs no model. Missing proof stays unverified; CI text is not proof CI actually ran.
See [review a work contract](#review-a-work-contract) and [contract format](docs/contracts.md).

## Why Kritic: Kev + critic

**Kritic = Kev + critic.** Kev is the optional local typed-decision engine; the critic is the
evidence-based review workflow. The name reflects that pairing, while the default workflow
works without Kev or any other model.

A **classifier model** learns to assign inputs to defined categories: for example, whether a
reported problem looks relevant, which issue type it belongs to, or whether there is enough
evidence to judge it. A score can help prioritize review; it is not proof the judgment is right.
Kev fills this bounded decision role. It uses a language model to return structured judgments,
rather than generating the implementation. Structured output alone does not make a model a
trained classifier or guarantee calibrated confidence.

This division suits code review: tests, scanners and browser checks collect concrete evidence;
an optional model helps interpret and prioritize it; the coding agent checks the claim and
decides what to fix. Narrow decisions are easier to validate than an unrestricted review essay,
and deterministic checks still work on a friend's laptop with no GPU. Models remain useful for
ambiguous cases, but can miss subtle bugs or flag correct code. The [examples below](#evidence-from-real-use)
demonstrate particular checks, not universal model accuracy.

**Why Rust?** Kritic coordinates processes, browser sessions, files and optional model resources.
Rust's type system and ownership help make those lifetimes explicit, catch invalid states during
development and avoid garbage-collector pauses in the server. A compiled executable also keeps
the minimum Windows installation small in dependencies: no Python or Node runtime is required
for the core. Optional tools still have their own dependencies. Rust supports a dependable
orchestrator; it does not make model judgments correct, sandbox commands or eliminate all bugs.

## Scoped commits and repository organization

The source branch adds a model-free `repository_critic`, staged-index commit budgets and an
explicit pre-commit hook. Repository-owned rules cover file LoC, naming, folder structure,
layer boundaries and explanatory-comment review. Scope plans bind an explicit purpose/file list
to the staged tree; agents still inspect semantic relevance and modularity independently.
See [setup and limitations](docs/repository-policy.md). These features are not in the older
published test-release binary.

## Experimental local semantic map

The source branch includes an opt-in CPU retrieval experiment; it is not bundled in the
current test-release binary. Five model variants were measured without a GPU. The smallest,
8-bit MiniLM-L3, used 17.5 MB of weights and about 286 MB sampled process memory, with a
65 ms warm-query p95 on the test machine. **None met the declared retrieval quality gate.**
The experiment therefore does not enable semantic evidence selection by default. See the
[measured comparison and limitations](docs/semantic-map.md#measured-results-2026-10-15).

## What your hardware can run

**A GPU unlocks optional local reviewers, not the deterministic checks.** Capabilities are
explicit configuration choices; Kritic does not automatically enable them because a GPU exists.
16 GB system RAM is a practical recommendation for an IDE/browser and ordinary projects, not a
measured minimum. Your project's builds, browsers and scanners can require more.

| Your machine / reviewer | Available with the right optional dependencies | Unavailable or limited |
|---|---|---|
| CPU, no dedicated GPU, no extra API key | Artifact checks, work contracts, process/resource checks; browser/UX, scanners, linters and Windows desktop checks after installing their dependencies. Your existing Codex/Work/Cursor session interprets evidence. | No Kritic model code scores, visual model judgments or model context helpers with backends off. Deterministic UX rules still work. |
| CPU + direct cloud API | Everything above plus optional text critic; visual critic/novice only with an image-capable API model and cloud vision enabled. | Requires separate provider API access, sends selected review inputs to that provider. No local GPU needed. |
| 8 GB GPU | All deterministic features; local Kev Q8 is a realistic testing target with about 5 GB recorded usage, reserving roughly 1.5–2 GB for display/system/apps plus slack. | Vision at 8 GB is tight and unverified. Other apps/context buffers may leave too little headroom. Mixed Q4 Kev is about 4.21 GB on disk, not a 2 GB model. |
| 12 GB+ GPU | Local Kev and vision are the recommended sequential testing configuration, in addition to deterministic tools. | Not a promise they fit concurrently or that local-model judgments are accurate. Other GPU users still consume memory. |
| CPU-only local Kev | Optional slower text review with `kev.cpu_only=true` and a CPU service; budget model RAM plus runtime overhead separately from the IDE/project. | No measured minimum RAM/latency guarantee. No CPU-only vision setup promise. |

Windows x86-64 is the tested platform. Desktop UI Automation needs Windows and an interactive
desktop; it cannot drive your PC from an unrelated cloud container. Linux/macOS core/browser
paths need field testing; there are no prebuilt packages for those platforms in this release.

## Quickstart: install the minimum, then choose extras

1. **Get the binary:** download the Windows ZIP from [the test release](https://github.com/TmRxJD/kritic/releases/tag/v0.5.0-test.3).
   Extract it into a persistent location and open its `kritic` folder in PowerShell.
   Install the [Microsoft Visual C++ x64 runtime](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170)
   if absent. No Rust, Python, Node, GPU, Ollama or API key is required for the minimum setup.
2. **Choose the minimal profile:** on a fresh installation run the commands below. If you already
   have `kritic.toml`, merge the profile deliberately instead of replacing your settings.
3. **Connect your agent:** follow [Codex / local Work, Cursor, Claude Code or Hermes setup](docs/clients.md).
   Install both the MCP connection and skill; one does not automatically install the other.
4. **Verify:** run `doctor`, then ask your agent to call `status(start=false)` and
   `artifact_validate(use_model=false)` on a small real change. Review its actual evidence.

```powershell
$env:KRITIC_HOME = (Resolve-Path .).Path
# Fresh install only: do not overwrite an existing config.
Copy-Item .\examples\profiles\minimal.toml .\kritic.toml
.\bin\kritic.exe setup --yes --backend off --no-vision --no-harness --no-scanners --no-register
.\bin\kritic.exe doctor
```

The minimal profile exposes artifact, contract and native resource/hygiene checks plus utility
tools. Browser/UX, security, quality, desktop, plugins and model helpers are deliberately hidden.
Installing a dependency does not turn its group on. Enabling a group does not install its dependencies.

### Add only the capabilities you need

Edit `$KRITIC_HOME/kritic.toml`, use [the complete configuration reference](examples/kritic.toml)
for the named keys, then restart your MCP connection. Keep `[review].backend` and
`[vision].backend` set to `"off"` for all the deterministic rows below.

| Optional capability | Enable in `[capabilities]` | Install / configure | What it gives you |
|---|---|---|---|
| Web interactions and UX rules | `web=true`, `ux=true` | Chrome/Edge/Chromium; app URL and useful workflow/spec. Run setup with `--no-scanners --no-register` to provision missing harness dependencies for enabled groups. | `ui_test`, `ux_critic(novice="off")`; interaction evidence and axe/workflow rules. No screenshot model score. No Node/Playwright required for native CDP. |
| Security | `security=true` | Install uv first if Python scanners are absent; setup uses `uv tool install` for Semgrep/Bandit/Ruff and provisions gitleaks/osv-scanner. Run setup with `--no-harness --no-register`. Network may be needed. | `security_critic`; missing applicable scanners remain incomplete coverage. cargo-audit is optional. |
| Quality | `quality=true` | Your project's linter/typechecker and its normal dependencies; Ruff where applicable. | `code_quality_critic`; Kritic does not install your project's toolchain. |
| Windows desktop / Unity | `desktop=true` (and `ux=true` for workflows) | Python 3.11+, uv, pywinauto harness; interactive Windows session. Run setup with `--no-scanners --no-register`. Unity checks also need the matching Unity editor. | `desktop_test`, `unity_tests`, desktop UX evidence. No GPU model required; the app under test may require one. |
| Repository checks / Beat Saber maps | `plugins=true` / `domain_maps=true` | Relevant repository plugin or map evaluator and its dependencies. | Project-defined checks / `map_eval`; not general model review. |
| Text model review | No capability group; select `[review].backend` | Optional [cloud API](docs/cloud.md) or [local Kev](docs/getting-started.md#optional-local-models). | `critic_score`, `decide`; advisory model assessments. |
| Image model review / novice | Set `[vision].backend`; `ux=true` for novice | Image-capable cloud API or local Ollama vision model; browser/desktop capture dependencies for the target. | `visual_critic`, model novice observations; independently validate them. |
| Model context helpers | `context=true` and text backend enabled | Same reviewer as text review. Hermes archival integration is separately configured. | `route`, `context_trim`, `locate`; no automatic context-engine takeover. |

Every optional setup command must also include `--yes --backend off --no-vision` for deterministic
use. Setup reads the capability switches and skips disabled groups. `--no-harness` skips both
browser and desktop provisioning; `--no-scanners` skips scanner provisioning; `--no-register`
prevents unrelated Claude/Hermes registration. These flags skip installs, not capability exposure.

`status` shows exposed tools/configuration. `doctor` checks dependencies, while `doctor --online`
explicitly makes a reviewer API/inference call. No models/API calls are needed just to install the skill.
Environment overrides take precedence over the file, including KRITIC_CONFIG and backend/capability
variables; verify the effective settings in status if your installed tools differ from the chosen profile.
For source builds and other platforms, follow [the source installation path](docs/getting-started.md#install-from-source-and-configure).

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

For explicit local-model installation commands and runtime/quantization limits, see
[optional local models](docs/getting-started.md#optional-local-models).
Local inference is optional: roughly 8 GB VRAM for Kev Q8, 12 GB recommended for sequential
Kev and vision. The 8 GB vision configuration remains a field-testing target, not a guarantee.
No GPU is required for deterministic checks or cloud review. 16 GB system RAM is a practical
recommendation; the tested project may require more. See [hardware and dependencies](docs/getting-started.md).

## Professional UI and showcase quality

Passing interaction or accessibility checks does not establish visual quality. The optional
visual critic now assesses typography, composition, identity, asset coherence and finishing
detail separately, against functional (6), professional (8, default) or showcase (9) anchors.
`aaa` selects the showcase target; it never certifies a whole product as AAA.

The [reference bank](specs/visual-reference-anchors.md) includes restrained task apps, data-heavy
dashboards, controller interfaces and expressive/minimalist games, plus competent-but-generic
counterexamples. You can supply two actual comparison screenshots with `reference_images`.
New model assessments are **uncalibrated and provisional**: `approval` is always `null`;
missing evidence stays unverified. See [visual quality and examples](docs/visual-quality.md).

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
python -m pip install pytest==9.1.1 PyYAML==6.0.3 ruamel.yaml==0.18.16
python -m pytest integrations/hermes/tests
python scripts/test_calibration_policy.py
```

Live model tests are opt in. Mock provider tests make no paid calls. Test fixtures deliberately
contain defective code and unfinished markers; they are not production implementation stubs.
Windows is the development platform; cross-platform support needs field confirmation. Do not
represent an unrun OS/provider/model combination as verified.

## Evidence from real use

- **A false positive corrected during the Tracker audit:** the first repository collector
  rejected a JavaScript file containing an intentional NUL string separator as binary.
  Independent `node --check` passed. Valid UTF-8 source is now audited with a warning;
  actual coverage failures stay explicit. Regression tests distinguish the two cases.
  This verifies that correction, not the architectural quality of the whole Tracker.
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
[Codex/Work and Cursor](docs/clients.md) ·
[Cloud reviewers](docs/cloud.md) · [Work contracts](docs/contracts.md) ·
[Tool behavior](docs/tools.md) · [Architecture](docs/architecture.md) ·
[Security audit](docs/security-audit.md) ·
[Test release](docs/testing-release.md) · [Historical measurements](docs/measurements.md)

Apache-2.0. Optional model runtimes, scanners, axe-core and other dependencies retain their own
licenses. Historical benchmark results describe the recorded configuration, not this release's
accuracy on arbitrary projects.

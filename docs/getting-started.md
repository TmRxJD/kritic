# Getting started — test release

Kritic runs deterministic checks by default. Your calling agent interprets the evidence using
its existing session; no local model, dedicated GPU or separate cloud API key is required.
Do not trust results without checking the underlying assertions and coverage.

## Requirements

- Windows x86-64 test binary: no Rust installation needed; it imports `VCRUNTIME140.dll` and
  needs the [Microsoft Visual C++ x64 runtime](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170). Source build: Rust 1.89+ and platform linker/build tools.
- Browser checks: installed Chrome, Edge or Chromium, driven through native CDP. Node and
  Playwright are not needed for the default native path; the legacy harness is retained for tests.
- Windows desktop automation: Python 3.11+ and uv for the included pywinauto harness.
- Resource/hygiene checks: native Rust implementation. The legacy Python harness is optional.
- Security: installed gitleaks, osv-scanner, semgrep, bandit and optional cargo-audit.
  Applicability/missing dependencies are reported; scanners may need network access.
- Quality: the project's own linter/typechecker toolchain. Unity tests need the matching Unity editor.
- Practical RAM recommendation: 16 GB for an IDE/browser and ordinary projects. There is no
  benchmarked universal RAM minimum; the application's build/tests may need more.

Windows is the development platform. Desktop UI Automation is Windows-only. Native browser
and core Rust functionality are intended to work on Linux/macOS; do not infer tested coverage
for those environments from the Windows measurements.

## Prebuilt Windows package

Use the configuration and examples from the same ZIP as the binary. Keys such as `capabilities.repository`, `capabilities.history` and `capabilities.debt` require
test.4 or later. Framing, staleness, invariants and project-aware setup recovery require test.5 or later; copying new keys into an older package does not add features.

Extract the test ZIP into your chosen persistent directory, open that `kritic` folder, and on
a fresh install select the included minimal profile before running setup:

```powershell
$env:KRITIC_HOME = (Resolve-Path .).Path
if (Test-Path .\kritic.toml) { throw "Existing config: preserve it and merge selected settings." }
Copy-Item .\examples\profiles\minimal.toml .\kritic.toml
.\bin\kritic.exe setup --yes --backend off --no-vision --no-harness --no-scanners --no-register
.\bin\kritic.exe doctor
```

Do not copy over an existing config: merge the selected settings instead. This first-run path
installs no optional harnesses/scanners/models and registers no unrelated agent. It exposes
artifact, contract and native process/resource checks. Your project's validators still need
their own normal toolchains. Connect [Codex/local Work, Cursor or another client](clients.md)
and install the skill; verify its TEST RELEASE header.

## Optional installs, in order

In KRITIC_HOME/kritic.toml enable only the groups you need. All commands below preserve
deterministic review and avoid automatic Claude/Hermes registration; restart MCP afterward.

| Goal | First edit `[capabilities]` | Then run from the extracted package |
|---|---|---|
| Web UI / deterministic UX | `web=true`, `ux=true`; leave `desktop=false` | `.\bin\kritic.exe setup --yes --backend off --no-vision --no-scanners --no-register` |
| Security scanners | `security=true` | `.\bin\kritic.exe setup --yes --backend off --no-vision --no-harness --no-register` |
| Windows desktop UI | `desktop=true`; optionally `ux=true` | `.\bin\kritic.exe setup --yes --backend off --no-vision --no-scanners --no-register` |
| Project code quality | `quality=true` | Install your project's lint/typecheck dependencies, then run doctor; no model/setup needed |
| Scoped commits / organization (test.4+) | `repository=true`; minimal profile defaults false | Use test.4 or later, configure `.kritic/repository.toml`, then follow [explicit hook setup](repository-policy.md); Git required, no model/download |
| Review history (test.4+) | `history=true`; built-in/minimal default false | Git plus private storage; [record selected reports/inputs explicitly](memory-adapter.md); no model, embedding or extra runtime |
| Technical-debt review (test.4+) | `debt=true`; built-in/minimal default false | No additional install, model or package-manager execution; run `kritic debt --base-dir .` offline. Public metadata needs a separate `--online` opt-in; [guide](technical-debt.md) |
| Test freshness / invariants (test.5+) | `staleness=true`, `invariants=true` | Define actual ownership, test and invariant evidence in `.kritic/test-contracts.toml`; [guide](staleness.md). No model or extra runtime |
| Debugging framing (test.5+) | `framing=true` | Define `.kritic/debug-case.json` with observations, competing hypotheses and traced evidence; [guide](framing.md). No model or extra runtime |
| Repository plugins / map domain | `plugins=true` / `domain_maps=true` | Install the relevant project plugin/evaluator dependencies; no model required for map_eval |

Browser provisioning uses installed Chrome/Edge/Chromium or downloads Chrome for Testing.
Install [uv](https://docs.astral.sh/uv/getting-started/installation/) before desktop provisioning
or scanner setup when Semgrep/Bandit/Ruff are absent: Kritic uses uv to prepare those Python tools.
Desktop also needs Python and the Windows harness; matching Unity is a separate application
install. Scanners can access networks. Partial install failures remain visible in
doctor and critic coverage; a successful setup command does not prove every optional dependency
or project validator works. Do not install local models to fix missing deterministic dependencies.

## Install from source and configure

Repository policy tools and commit hooks described in [repository policy](repository-policy.md)
require test.4 or later.
Installing or registering the server never installs a Git hook automatically.

```bash
git clone https://github.com/TmRxJD/kritic.git
cd kritic
export KRITIC_HOME='E:/AI/kritic' # optional; choose your own persistent data directory
scripts/install.sh
cp examples/profiles/minimal.toml "$KRITIC_HOME/kritic.toml" # fresh install only
"$KRITIC_HOME/bin/kritic" setup --yes --backend off --no-vision --no-harness --no-scanners --no-register
```

On Windows the binary is `kritic.exe`. Without KRITIC_HOME it lives in `~/.kritic/bin`.
The installer builds the binary, copies the optional Python harness, and writes a commented
config if absent. It does not pull models by default. Setup discovers/installs missing tools;
use `--no-harness --no-scanners --no-register` to skip dependency installation/registration.
Existing config selections are preserved unless you explicitly override them.

The source installer requires Rust 1.89+ and native linker/build tools; prebuilt Windows does not.
Clone the repo and run these commands from its root. On Windows use `bin/kritic.exe`.
Use [client-specific setup](clients.md) for Codex/local Work, Cursor, Claude Code and Hermes.
`kritic doctor` reports configured/missing features;
`--online` explicitly tests the configured reviewer and may incur API cost.

## Optional technical-debt review

Technical-debt review does not need a local model. After building a matching source binary,
enable `[capabilities] debt=true`, restart the client and check discovery for
`technical_debt_critic`. Start offline. `kritic debt --online` authorizes public package
metadata lookup; `kritic doctor --online` probes the configured reviewer and is a different
action. No source content or environment-variable values are needed for dependency lookup.

## Optional local models

```bash
kritic setup --yes --backend local --no-vision --no-harness --no-scanners --no-register
# Separate opt-in: installs/starts Ollama and pulls the vision model where supported.
kritic setup --yes --vision --no-harness --no-scanners --no-register
```

Kev Q8's recorded footprint is about 5 GB: recommend an 8 GB GPU after allowing roughly
1.5–2 GB for desktop/display/browser use plus slack. A 6 GB GPU is borderline. Vision has a
6 GB download plus runtime buffers: 8 GB is tight, 12 GB recommended. Kritic coordinates
one selected local model at a time; other applications still consume VRAM.

The available mixed Q4 Kev file is about 4.21 GB versus Q8's 4.50 GB; it is not a 2 GB model.
Changed or unverified quantization is advisory. Local gates require runtime health metadata
matching the configured model, Q8 quantization and GGUF path; older health endpoints stay unscored
for approval while still returning advisory model assessments. CPU-only Kev is an optional, slower path
(`kev.cpu_only=true`); no CPU latency promise is made. No local inference is launched by default.

Cloud and existing-agent alternatives: [cloud reviewers](cloud.md).

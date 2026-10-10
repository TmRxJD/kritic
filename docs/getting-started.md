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

Extract the test ZIP into your chosen persistent directory, open that `kritic` folder, and run:

```powershell
$env:KRITIC_HOME = (Resolve-Path .).Path
.\bin\kritic.exe setup --backend off --no-vision
.\bin\kritic.exe doctor
```

This uses the included binary; no Rust or local model is needed. Setup can install deterministic
dependencies and register supported agents. `--no-harness --no-scanners --no-register` skips those
steps. Install the bundled skill in your host's skill directory, and verify the TEST RELEASE header.

## Install from source and configure

```bash
export KRITIC_HOME='E:/AI/kritic' # optional; choose your own persistent data directory
scripts/install.sh
"$KRITIC_HOME/bin/kritic" setup --backend off --no-vision
```

On Windows the binary is `kritic.exe`. Without KRITIC_HOME it lives in `~/.kritic/bin`.
The installer builds the binary, copies the optional Python harness, and writes a commented
config if absent. It does not pull models by default. Setup discovers/installs missing tools;
use `--no-harness --no-scanners --no-register` to skip dependency installation/registration.
Existing config selections are preserved unless you explicitly override them.

Register Claude Code or Hermes with `kritic register --claude` / `--hermes`. Other clients can
start the binary with `serve`; copy the shipped skill into their skill directory. Do not assume
Claude's paths work in every client. `kritic doctor` reports configured/missing features;
`--online` explicitly tests the configured reviewer and may incur API cost.

## Optional local models

```bash
kritic setup --backend local --no-vision
kritic setup --vision
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

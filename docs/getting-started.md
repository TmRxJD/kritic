# Getting started

## What you need

kritic is one Rust binary plus optional pieces. Each tool needs only its own pieces; anything
missing makes that tool report `approval: null` with the reason. It never reports a pass it
didn't check.

| Feature | Tools | Needs |
|---|---|---|
| Core critic | `critic_score`, `artifact_validate`, `status`, `spec_lint`, `route`, `context_trim`, `token_metrics`, `plugin_list`, `plugin_run` | the kritic binary; **laya** + the **Kev-4B GGUF** for the model parts (`artifact_validate` with `use_model=false` and the plugins need neither) |
| Web UI testing | `ui_test`, `ux_critic` (web) | Node 18+, the harness's npm packages, Playwright Chromium |
| Visual review | `visual_critic`, the `ux_critic` novice | [Ollama](https://ollama.com) with `qwen2.5vl:7b` (about 6 GB) |
| Security | `security_critic` | any of gitleaks, osv-scanner, semgrep, bandit, cargo-audit (on PATH or in `scan.tools_bin`); network access for osv-scanner and semgrep's rule download |
| Code quality | `code_quality_critic` | the project's own linters: ruff, clippy, eslint, tsc |
| Desktop and processes | `desktop_test`, `resource_critic`, `hygiene_critic`, `ux_critic` (desktop, Unity) | Python 3.11+ through [uv](https://docs.astral.sh/uv/); `desktop_test` is Windows-only (UI Automation) |
| Unity | `unity_tests`, `desktop_test` Unity mode | a Unity Hub editor install matching the project |
| Beat Saber maps | `map_eval` | nothing extra; disabled by default (`[tools] disabled`) |

**Platform.** kritic is developed and measured on Windows 11 with an RTX 5070 (12 GB). The Rust
server, the web harness and the model runtimes are cross-platform; the desktop tester is
Windows-only. Linux and macOS builds are not yet tested: reports welcome.

**GPU.** Kev-4B Q8_0 needs about 5 GB of VRAM; the vision model about 6 GB. kritic keeps only one
of them on the GPU at a time (see [architecture](architecture.md#gpu-arbitration)), so an 8 GB card
works. laya also has Vulkan and Metal builds. `ud_q4_k_m` is a smaller Kev quantization; the shipped
gates were calibrated on Q8_0.

## Install

```bash
git clone https://github.com/TmRxJD/kritic
cd kritic
scripts/install.sh --with-models
```

`install.sh` builds the release binary and installs it into `~/.kritic` (or `$KRITIC_HOME`):

- `bin/kritic[.exe]`: the MCP server;
- `bin/harness/`: the Playwright and desktop harnesses, with their npm and uv dependencies;
- `kritic.toml`: your config, copied from [`examples/kritic.toml`](../examples/kritic.toml).

`--with-models` also runs `scripts/fetch-models.sh`, which downloads:

- **laya**, the Kev inference server, from [monatis/ggmlc releases](https://github.com/monatis/ggmlc/releases).
  It picks `cuda-sm89` when `nvidia-smi` exists (it JIT-compiles for newer GPUs), else `vulkan`, or
  `metal` on macOS. Override with `--backend`. On Windows CUDA it also fetches the CUDA 12 runtime
  and cuBLAS DLLs from NVIDIA's wheels.
- **Kev-4B** from [mys/kev-4b-GGUF](https://huggingface.co/mys/kev-4b-GGUF) (4.5 GB for Q8_0).

The vision model is separate:

```bash
ollama pull qwen2.5vl:7b
```

Scanners are optional. For example:
[gitleaks](https://github.com/gitleaks/gitleaks/releases),
[osv-scanner](https://github.com/google/osv-scanner/releases),
`uv tool install semgrep bandit ruff`, and `cargo install cargo-audit`.

## Register with Claude Code

```bash
claude mcp add --scope user kritic -- ~/.kritic/bin/kritic
```

Use the absolute path on Windows (`C:/Users/<you>/.kritic/bin/kritic.exe`). Then copy the skill, the
UX agent and the CLAUDE.md rule so Claude uses kritic at the end of each task:

```bash
cp -r examples/claude/skills/kritic ~/.claude/skills/
cp examples/claude/agents/ux-critic.md ~/.claude/agents/
cat examples/claude/CLAUDE.md >> ~/.claude/CLAUDE.md
```

Any MCP client works: kritic speaks MCP over stdio. [`examples/mcp.json`](../examples/mcp.json)
is a project-scoped `.mcp.json`.

## Check it

Start a new Claude Code session and ask it to call kritic's `status` tool with `start=true`. It
reports whether laya is up, which paths it resolved, the config file it read and any errors in it,
every calibrated gate, and the live event stream URL. The first Kev call after laya starts takes
about 40 s while CUDA JIT-compiles its kernels; the compiled kernels are cached in
`kev.cuda_cache`, so later starts take about 13 s.

Next: [configuration](configuration.md), [tools](tools.md), [Claude Code integration](claude-code.md).

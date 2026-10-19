# Changelog

## Unreleased

- **Policy runner** (`policy_schema`, `policy_run`, `trajectory_diagnose`): a game-agnostic frame loop over a documented framed-buffer protocol (loopback TCP or stdio), schema-driven state/action decoding, pluggable policies (defer, scripted, external process, Kev at low frequency), `lockstep` and `hold` modes with per-tick latency against the native budget, JSONL telemetry, and trajectory diagnostics: behavior families, stuck states, oscillation, unrecoverable metrics, and a veto of 0%-success families with their coordinate signatures. See [docs/policy-runner.md](docs/policy-runner.md).
- **Docs**: [other testing uses for Kev](docs/kev-testing-uses.md).

## 0.4.0 — 2026-09-29: kritic

First public release, under the name **kritic** ("k" for Kev). Earlier versions (0.1–0.3) were a
private, single-machine tool with the working name *jev_governance*.

- **Renamed** throughout: crate and binary `kritic`, MCP server `kritic`, environment variables
  `KRITIC_*`, repo folder `.kritic/` (plugins and `ux.toml`), skill `kritic`.
- **Config file**: `kritic.toml` (`$KRITIC_CONFIG`, else `$KRITIC_HOME/kritic.toml`) with every
  setting; environment variables still override it. `status` reports the file read and any errors.
- **No machine-specific defaults**: everything defaults under `~/.kritic` or to PATH lookups. laya is
  found in `~/.kritic/laya/` or one folder below; its log moved to the log directory.
- **`[tools] disabled`**: hide tools from the client. `map_eval` (Beat Saber) is off by default.
- **Install scripts**: `scripts/install.sh` (binary + harness + config) and
  `scripts/fetch-models.sh` (laya for CUDA/Vulkan/Metal, Kev-4B GGUF, Windows CUDA runtime DLLs).
- **Unity `ux_critic`**: `app.shot_dir` replaces an app-specific environment variable.
- **Calibration scripts** take `--corpus`, `--history` and `--maps` instead of fixed paths.
- **Benchmark** (`bench/`): 14 defect/clean case pairs reviewed without and with kritic.
- **Docs and examples**: `docs/`, `examples/` (config, Claude Code skill/agent/rule, a repo plugin,
  `ux.toml` for web and Unity).

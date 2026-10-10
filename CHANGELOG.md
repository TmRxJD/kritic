# 0.5.0-test.1 (prepared; not published)

- Test-release skill: independently verify critic claims, evaluate workflow viability, reproduce
  and fix Kritic defects with regressions and upstream PRs.
- Deterministic-by-default setup/config; local models and cloud reviewers explicitly optional.
- OpenAI/OpenAI-compatible, Anthropic, Gemini and JEV transports; optional cloud vision,
  strict bounded responses, redacted errors, provider-origin-bound saved credentials.
- Work contracts: declared acceptance checks, coverage gaps, current input hashes, stale evidence,
  final revalidation, separately reported static enforcement wiring and prose sources.
- Capability/tool toggles, no GPU side effects for cloud/off, scoped calibration provenance,
  rejection of empty/skipped/vacuous approvals, synchronized docs and offline CI.

# Changelog

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

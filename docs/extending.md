# Extending kritic

**For project-specific evaluators, use a repo plugin instead of changing kritic.** Drop a manifest
in `<repo>/.kritic/plugins/` and it is available through `plugin_list` / `plugin_run`; see
[plugins](plugins.md). Change kritic itself only for capabilities every project needs.

Follow this contract so the tool behaves like the others.

## Rules

1. **Real tools produce findings; models only score.** Kev (`Decider`) answers typed questions
   (noul / choice / score) about facts your tool gathered. It never invents a finding, and a model
   opinion must never *remove* a deterministic failure.
2. **Approval is computed in code** from stated thresholds. Per-call thresholds are tool parameters;
   global settings go in `Config` (`src/config.rs` `KEYS`, `examples/kritic.toml`, `docs/configuration.md`).
3. **Never fabricate.** If a dependency is missing or offline, the tool is `skipped`/`failed` and
   approval is `null`, with the reason in the result. Never default to a clean result.
4. **Keep responses small.** Save the full result with `tools::save_full` (`full_result`) and trim
   lists to `tools::MAX_ITEMS` in the MCP response.
5. **Take the GPU lock** (`self.gpu("<tool>").await`) around any Kev or vision call. It
   serializes across processes (`runs/gpu.lock`), so concurrent sessions never co-load models.
6. **Emit lifecycle and progress.** `started`/`finished` in the wrapper; `crate::events::emit("progress", ..)`
   for long loops; harness subprocesses print `{"event": ...}` JSON lines on stderr, and
   `proc::run_streaming` forwards them to the WebSocket.
7. **Safety defaults.** Only touch local targets unless explicitly allowed, skip destructive
   actions unless allowed, and redact secrets.

## Where things go

- Logic: a module in `src/` with a pure `judge`/`aggregate` function, so it's testable without I/O.
- Types: `#[derive(Serialize, schemars::JsonSchema)]` result struct (output schema is generated).
- MCP wrapper: `src/server.rs` inside `#[tool_router] impl Kritic`: params struct with
  doc comments (they become the tool's input schema), `started` / `finished` / `audit`.
- Tests: offline tests with a fake `Decider` in `tests/`; live tests gated on `KRITIC_LIVE=1`.
- Docs: `docs/tools.md`, and `examples/claude/skills/kritic/SKILL.md` if agents should use it by default.

## Build and test

```bash
cargo test --test unit --test phase2 --test phase3 --test metrics --test specs --test plugins --test config --test ux
cargo clippy --all-targets -- -D warnings
(cd harness && npm test)                       # web harness against its fixtures
(cd harness/py && uv run --extra test pytest -q)   # desktop harness (Windows)
KRITIC_LIVE=1 cargo test --release --test live --test phase2 --test phase3 -- --nocapture --test-threads=1   # real models and tools
KRITIC_LIVE=1 KRITIC_UX_NOVICE=vision cargo test --release --test ux -- --nocapture --test-threads=1       # ux_critic calibration
scripts/install.sh                             # install the build into ~/.kritic
```

Each client session runs its own copy of the server, and Windows locks a running exe: `install.sh`
renames the old binary aside, so running sessions keep it and new sessions get the new build.

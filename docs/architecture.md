# Architecture

```
            MCP client (Claude Code, …)
                     │  stdio, JSON-RPC
┌────────────────────▼─────────────────────────────────────────────┐
│ kritic (Rust, rmcp)                                              │
│  server.rs   tool router, params → schemas, lifecycle events     │
│  critic.rs   chunk / diff → Kev specs → calibrated gates          │
│  checks.rs   parsers, merge markers, project commands            │
│  scan.rs     gitleaks · osv-scanner · semgrep · bandit · cargo-audit · linters │
│  phase3.rs   security / quality / resource / desktop orchestration │
│  ui.rs ux.rs vision.rs unity.rs plugins.rs hygiene.rs map.rs     │
│  spec.rs     decision specs: schema, lint, state validation       │
│  kev.rs      laya client, batching, truncation detection         │
│  gpu.rs      one model on the GPU, across processes              │
│  events.rs   WebSocket event stream                              │
│  usage.rs metrics.rs   token accounting                          │
└──┬──────────────┬───────────────┬────────────────┬───────────────┘
   │ HTTP         │ HTTP          │ JSON lines      │ processes
   ▼              ▼               ▼                 ▼
 laya + Kev-4B  Ollama + VL    harness/          scanners, linters,
 (typed          (visual,       ui_test.mjs       project commands,
  decisions)      novice)       ux_web.mjs        repo plugins
                                py/desktop_test.py
                                py/ux_adapter.py
```

## Flow of a verdict

1. **Facts first.** Deterministic code produces every finding: parsers, commands, scanners, a real
   browser, UI Automation, process sampling, or a repo plugin.
2. **Kev scores.** Where a judgment is needed, a [decision spec](decision-specs.md) turns facts into
   a declared state. Kev answers typed questions about it with probabilities. It only ranks findings
   or scores measurements; it never adds a finding or removes a deterministic failure.
3. **Code decides.** Approval is computed from stated thresholds: deterministic failures, budgets,
   and Kev gates that carry dated calibration evidence. Anything unmeasured makes approval `null`.

## GPU arbitration

Kev (laya) and the vision model (Ollama) never share the GPU. Measured on a 12 GB card, both
resident beside a desktop made Windows spill VRAM into system RAM, and a vision call went from 1.3 s
to 66 s. Every model-backed tool, and the startup warm-up, holds one lock:

- in-process: a mutex;
- across processes: an OS file lock on `<runs_dir>/gpu.lock`, since each client session runs its
  own kritic. `gpu.owner` names the holder. The lock is released automatically if the holder crashes.
- Kev tools unload the vision model first (`keep_alive: 0`). `visual_critic` and the vision novice
  stop laya first; laya restarts in about 13 s on the next Kev call, thanks to the CUDA JIT cache.

After `server.gpu_wait_s` (900 s) a waiting tool runs anyway rather than failing, and reports
`gpu: contended`.

## Processes and cleanup

Harness subprocesses run in their own process group and are killed with their tree on timeout.
`desktop_test` and `resource_critic` snapshot processes and listening ports before launching, then
report and kill what the run left alive. `hygiene_critic` finds leftovers from earlier runs:
orphans, sockets held by inherited handles, and leftover files.

## Files

| Path | What |
|---|---|
| `<home>/logs/calls.jsonl` | audit log of every tool call (inputs trimmed, verdicts) |
| `<home>/logs/model_usage.jsonl` | Kev and vision token counts and speeds |
| `<home>/logs/laya.log` | laya's output when kritic started it |
| `<home>/logs/ws/<pid>.url` | event-stream URL of each running server |
| `<home>/runs/<stamp>-<tool>/` | screenshots and `result.json` (`full_result`) per run |
| `<home>/runs/gpu.lock`, `gpu.owner` | the cross-process GPU lock |

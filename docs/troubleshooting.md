# Troubleshooting

Start with `status(start=true)`. It shows whether laya is up, every resolved path (and whether it
exists), the config file read, any `config_errors`, and the ready states.

| Symptom | Cause / fix |
|---|---|
| `laya unreachable` / `did not answer within 120s` | kritic starts `kev.laya_exe` itself. Loading takes about 60 s, and the first call after a start adds about 40 s of JIT compile. Retry once. Log: `<home>/logs/laya.log`. |
| `laya … not found` / `Kev GGUF not found` | Run `scripts/fetch-models.sh`, or set `kev.laya_exe` / `kev.gguf`. |
| `cublas64_12.dll` missing (Windows) | The CUDA 12 runtime DLLs must sit beside `laya.exe`. `fetch-models.sh` copies them from NVIDIA's `nvidia-cublas-cu12` and `nvidia-cuda-runtime-cu12` wheels. |
| `kev request timed out` | The GPU is saturated (a game or another model is loaded). Retry later, or report the critic as offline. |
| slow (> 5 s per chunk) | Another app is using VRAM, or `kev.token_budget` was raised above about 3,000. |
| `truncated_chunks > 0` | Lower `kev.chunk_chars` (default 1300). |
| `approval: null` from `critic_score` | Kev offline or coverage partial (`chunks_unscored`, over 60 chunks). Score the change in file groups. |
| `security_critic` shows a tool as `failed` / approval null | The scanner isn't installed (it is looked up in `scan.tools_bin`, then PATH), or it needs network: osv-scanner queries the OSV API, and semgrep downloads rulesets the first time. |
| `visual_critic` takes 60–90 s | The first vision call after Kev loads the ~6 GB model. Later calls take about 1–2 s. |
| `model_ok: false` (visual) | Ollama isn't reachable and `vision.ollama_exe` couldn't start it, or `qwen2.5vl:7b` isn't pulled (`ollama list`). |
| First Kev call after a visual review takes ~15 s | Expected: `visual_critic` stops laya to free the GPU. |
| `ui_test` error `refusing non-local URL` | By design. Test the local dev server, or pass `allow_remote=true` only for your own staging. |
| `ui_test` error `cannot start node` / harness not found | Node must be on PATH (or `ui.node`), and the harness needs `npm install` plus `npx playwright install chromium`. `install.sh` does both. |
| `ui_test` `no_effect` on a control that is meant to be inert | Hover-only or purely visual controls trip it. Pass `ignore`. |
| `desktop_test`: `no window appeared` | Wrong exe, a splash window comes first (set `window_title_re`), or the app needs `cwd`. |
| `desktop_test`: a control is `skipped: no usable UIA pattern` | A custom control without UI Automation support. `use_mouse=true` falls back to real clicks (this moves the cursor). |
| `desktop_test` Unity: stalls, then `stall_s` error | The app sent no event, wrote no log and accepted no socket connection for `stall_s`. Check `ready_event` and `app_events`, and that the port isn't held by a previous run (`hygiene_critic`). |
| frame timing `unavailable` | PresentMon needs an elevated process, or membership in **Performance Log Users** followed by a sign-out and sign-in. |
| `resource_critic` rejects a process you believe is fine | A garbage-collected runtime can grow its heap for a while before collecting. Run longer (`duration_s`), or raise `max_rss_slope_mb_per_min` for that workload, and say why. |
| Kev flags clean code | See [benchmarks](benchmarks.md): Kev's gates also reject some correct changes. Read the flagged spot; if you can't name a problem, report the flag as a false positive. Code whose strings or comments *describe* bugs, vulnerabilities or reviewer instructions draws flags too. |
| The session compacts right after it starts | Not kritic: Claude Code's `autoCompactWindow` is below the startup context. See [Claude Code](claude-code.md#context-use). |

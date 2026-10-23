# M1 native MLX validation — 2026-10-22

Scope: protocol integration and a short-context smoke test, not model quality, calibration,
production suitability or backend parity. Kritic main baseline: `37b000910bf4e3328fab1c8e88ce48dc93eb1e00`.
Upstream Kev: `2074cc44256200bb37540103fa4572901faa10f3`.

## Machine and model

Apple M1 Mac mini (`Macmini9,1`), 8 GiB unified RAM, macOS 26.5.2 (25F84),
128 GiB available disk before setup. Initial system memory-pressure report: 34% free.
Python 3.12.15, MLX 0.32.2, mlx-lm 0.31.3, torch 2.8.0; Rust 1.99.1.
Only `jaredpalmer/kev-0.8b@bf75a6a8848ea6960ff2ed108d9ed44c2941174f`
and its Qwen3.5-0.8B-Base dependencies were fetched. `/v1/models` confirmed
that checkpoint, the 0.8B base, `backend=mlx`, `device=mps`, `dtype=bfloat16`.
No 4B weights were fetched or loaded.

## Measurements

Decimal bytes below; footprint, RSS and Metal allocator values overlap and must not
be added. Footprint came from macOS `proc_pid_rusage` v4 (SDK-defined fields), also
cross-checked with `footprint -p`; RSS sampled every 200 ms with psutil. Metal values
came directly from `mlx.core.get_active_memory/get_peak_memory` in the upstream
service process, with an observation wrapper that did not change inference.

| Observation | Measured |
|---|---:|
| First startup, including initial imports/downloads | 128.08 s |
| Cached-weight startup to server construction | 11.86 s |
| Cached-start lifetime peak physical footprint, before requests | 3,236,536,576 bytes (3.01 GiB) |
| Idle physical footprint after startup | 2,021,923,520 bytes (1.88 GiB) |
| Physical footprint after four short requests | 2,227,608,448 bytes (2.07 GiB) |
| Metal allocator startup peak | 2,572,572,364 bytes |
| Metal active after load / after requests | 1,504,791,240 / 1,527,696,074 bytes |
| Sampled RSS peak, first / cached startup | 1,222,311,936 / 1,080,885,248 bytes |
| RSS after four short requests | 91,750,400 bytes (compressed/mapped memory makes RSS incomplete) |

The live Kritic test sent three typed questions (`noul`, `choice`, `score`) about
“The sky is blue.” The response reported 39 input tokens, all requested answers,
and no truncation. After cached startup and with Rust builds finished, wall-clock
latencies including readiness HTTP were **491, 60, 60, 61 ms**. The last three reuse
the exact state/prefix cache; these are four smoke samples, not a general benchmark.
An earlier first request during compilation took 11,086 ms, followed by 300, 93,
81 ms. Do not treat cached short-state latency as cold-start or long-document latency.
No model probabilities are presented as accuracy or reliability measurements.

## Checks and limitations

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo test --all-targets`: passed on macOS, with Python 3.12 on PATH. The suite
  launched 47 harnesses. Two existing verified-process sampling tests are explicitly
  ignored on macOS because the implementation supports Windows/Linux only; the
  Windows-only inherited-socket fixture is now also scoped to Windows.
- `cargo test --test kev_mlx`: six loopback contract tests passed; the real-inference
  test is ignored by default. Success, HTTP errors/redirect rejection, malformed
  answers, missing/extra IDs, model/base/checkpoint/backend/precision/device mismatch,
  strict timeout, service preservation, context rejection and advisory gates covered.
- `KRITIC_CONFIG=examples/profiles/mac-mlx-0.8b.toml cargo test --test kev_mlx
  live_mlx_0_8b -- --ignored --nocapture`: passed, then repeated directly using the
  built harness for isolated timing.
- Fresh model-off home: CLI help and `doctor --json` exited 0; MCP initialize,
  tools/list and status passed. Sixteen deterministic tools listed; `decide` absent,
  backends off, model readiness false and gates uncalibrated.
- Native profile: `doctor --online --json` exited 0; MCP initialize/list/status and
  a real `decide` call passed, reporting `kev_ok=true`, `advisory=true`,
  `local_ready=true`, `laya_up=false`, `gates_calibrated=false`.
- Both pinned profiles parse without inference/downloads; identity changes with the
  checkpoint/base/dtype and stays uncalibrated. The launcher refuses 4B on this 8 GiB
  Mac before loading, and refuses an occupied port before spawning a service.
- Initial full-suite attempts exposed a missing `python` alias (resolved by putting
  the existing Python 3.12 environment on PATH), then the two unsupported sampler
  tests. Initial Clippy found an unused read amount in the new fixture (fixed) and
  an existing Windows fixture constant compiled on macOS (scoped correctly).
- Other default ignores: external real-gitleaks test, explicit browser-effects test,
  and child-process fixture. Windows commands/desktop automation, CUDA/laya runtime,
  larger weights, production quality, backend parity and fresh calibration were not
  exercised. Existing live laya tests were not enabled with `KRITIC_LIVE`.

See [setup, configuration and troubleshooting](mac-mlx.md). All native MLX identities
remain unverified and advisory, including 4B. Temporary owned services were stopped;
unrelated services were preserved.

## CI security follow-up

The first PR run (`39882375459`) passed Semgrep, Windows Rust and legacy-browser,
but Bandit 1.9.4 failed on four low-severity/high-confidence findings in the original
launcher: B404 (`subprocess` import), B607 (PATH-resolved `sysctl`) and two B603
process calls. The downloaded job artifact contained no parser errors. No rules,
thresholds, scan targets or suppression comments were changed.

The launcher now generates an exclusive, mode-0600 launchd plist instead of executing
programs. RAM comes from `os.sysconf`; fixed argv, an absolute operator-selected venv
path, loopback host, explicit environment, null stdin, file logs and non-restarting
background ownership are declared in the plist. Startup/shutdown use explicit
`launchctl bootstrap`/`bootout` commands. Four model-free unit regressions verify the
job, memory/profile restrictions, occupied-port refusal, serialization, permissions
and no-overwrite behavior, and run in CI. A real launchd lifecycle smoke used a
fixture-only HTTP listener on 127.0.0.1:18009, verified arguments/environment/logging,
and stopped only that owned job. A first fixture under Documents was denied by macOS
privacy protection at Python initialization; the same fixture under the normal Projects
checkout passed. Setup documentation now requires an unprotected project directory for
launchd inputs/logs. No model weights were loaded for this follow-up.
Previous inference and memory measurements above describe the unchanged upstream
MLX runtime; the revised launchd path has been lifecycle-tested with the fixture,
not rebenchmarked with model weights.

Native score parsing also now rejects swapped/mismatched textual legend labels,
with a negative response regression. Bandit 1.9.4 and Semgrep 1.178.0 p/default both
pass locally with zero findings/errors; the existing nonempty-coverage verifier also
passes. Targeted Rust tests, format and strict all-target Clippy pass.

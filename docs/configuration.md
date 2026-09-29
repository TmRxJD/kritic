# Configuration

Every setting resolves in this order:

1. an environment variable (`KRITIC_*`), for example on the MCP entry;
2. the config file;
3. the built-in default.

The config file is `$KRITIC_CONFIG` if set, else `$KRITIC_HOME/kritic.toml`. `KRITIC_HOME` defaults
to `~/.kritic`, which is also where models, logs and run artifacts go by default.
[`examples/kritic.toml`](../examples/kritic.toml) lists every key, commented out with its default;
a test keeps it in sync with the code.

The `status` tool reports `config_file` (the file it read) and `config_errors`: bad TOML, unknown
keys, or an explicit `KRITIC_CONFIG` that doesn't exist. A bad key falls back to its default; it
never stops the server.

## Keys

| Key | Env | Default | Meaning |
|---|---|---|---|
| `kev.laya_url` | `KRITIC_LAYA_URL` | `http://127.0.0.1:<laya_port>` | laya endpoint |
| `kev.laya_port` | `KRITIC_LAYA_PORT` | 8089 | port laya is started on |
| `kev.laya_exe` | `KRITIC_LAYA_EXE` | `laya` on PATH, else `~/.kritic/laya/` | started when laya isn't running |
| `kev.gguf` | `KRITIC_KEV_GGUF` | `~/.kritic/models/kev_4b_q8_0.gguf` | Kev weights |
| `kev.model` | `KRITIC_KEV_MODEL` | `kev-4b` | model name sent to laya |
| `kev.cuda_cache` | `KRITIC_CUDA_CACHE` | `~/.kritic/cuda-cache` | persisted CUDA JIT (saves ~40 s per start) |
| `kev.chunk_chars` | `KRITIC_CHUNK_CHARS` | 1300 | code per Kev request; Kev's context is 2,048 tokens |
| `kev.token_budget` | `KRITIC_TOKEN_BUDGET` | 2900 | rows × tokens per laya batch; latency falls off a cliff above ~3.7k |
| `kev.max_seq_tokens` | `KRITIC_MAX_SEQ_TOKENS` | 2048 | used to detect laya's silent truncation |
| `kev.request_s` | `KRITIC_KEV_REQUEST_S` | 60 | one Kev request; a stall stops laya so the next call restarts it |
| `kev.critic_deadline_s` | `KRITIC_CRITIC_DEADLINE_S` | 600 | a whole critic pass returns what it has after this |
| `vision.ollama_url` | `KRITIC_OLLAMA_URL` | `http://127.0.0.1:11434` | |
| `vision.ollama_exe` | `KRITIC_OLLAMA_EXE` | `ollama` on PATH, else the platform install path | started when Ollama isn't running |
| `vision.model` | `KRITIC_VISION_MODEL` | `qwen2.5vl:7b` | |
| `vision.visual_min` | `KRITIC_VISUAL_MIN` | 6.0 | lowest per-criterion visual score (0–10) that approves |
| `review.max_revisions` | `KRITIC_MAX_REVISIONS` | 2 | revise-and-rescore rounds (`revise_allowed` turns false after) |
| `review.command_timeout_s` | `KRITIC_COMMAND_TIMEOUT` | 300 | each `artifact_validate` command |
| `review.context_limit` | `KRITIC_CONTEXT_LIMIT` | 1000000 | context size `token_metrics` measures against |
| `ui.node` | `KRITIC_NODE` | `node` | |
| `ui.harness` | `KRITIC_HARNESS` | `harness/ui_test.mjs` beside the binary, else in the source tree | |
| `ui.playwright_browsers` | `KRITIC_PLAYWRIGHT_BROWSERS` | Playwright's default | sets `PLAYWRIGHT_BROWSERS_PATH` |
| `ui.timeout_s` | `KRITIC_UI_TIMEOUT` | 300 | |
| `scan.tools_bin` | `KRITIC_TOOLS_BIN` | `~/.kritic/bin` | scanners are looked up here, then on PATH |
| `scan.cargo_tools_bin` | `KRITIC_CARGO_TOOLS_BIN` | `~/.cargo/bin` | cargo-audit |
| `scan.timeout_s` | `KRITIC_SCAN_TIMEOUT` | 600 | each scanner |
| `scan.semgrep_config` | `KRITIC_SEMGREP_CONFIG` | `p/default` | semgrep ruleset |
| `scan.max_triage` | `KRITIC_MAX_TRIAGE` | 25 | findings Kev ranks (ordering only) |
| `scan.max_hotspots` | `KRITIC_MAX_HOTSPOTS` | 10 | complex functions Kev rates |
| `desktop.uv` | `KRITIC_UV` | `uv` | runs the Python harness |
| `desktop.harness_py` | `KRITIC_HARNESS_PY` | `harness/py` beside the binary, else in the source tree | |
| `unity.editor` | `KRITIC_UNITY_EDITOR` | none | force one `Unity.exe` for `unity_tests` |
| `unity.hub_editors` | `KRITIC_UNITY_HUB_EDITORS` | the platform's Unity Hub editor folder | `<version>/Editor/Unity.exe` is used |
| `server.ws_port` | `KRITIC_WS_PORT` | 8790 | event stream; a free port is used if it's taken |
| `server.gpu_wait_s` | `KRITIC_GPU_WAIT` | 900 | how long to wait for another session's model |
| `server.log` | `KRITIC_LOG` | `~/.kritic/logs/calls.jsonl` | audit log; `model_usage.jsonl` and `laya.log` sit beside it |
| `server.runs_dir` | `KRITIC_RUNS_DIR` | `~/.kritic/runs` | screenshots, full results, the cross-process GPU lock |
| `tools.disabled` | `KRITIC_DISABLED_TOOLS` (comma-separated) | `["map_eval"]` | tools not offered to the client |

## What is not configurable

**Kev thresholds.** Every Kev question that can fail approval is a calibrated gate stored with its
evidence in `specs/*.json`: the date, the corpus, the separation it measured and the good-case
tail. Editing a number there would make that evidence false. To move a gate, recalibrate on your
own code; see [decision specs](decision-specs.md#calibration).

Deterministic budgets (complexity, memory growth, visual minimum, UX budgets) are parameters of
each tool call or of the repo's own spec files, not global settings.

## Shrinking the tool list

Every enabled tool's schema sits in the agent's context for the whole session. Disable what you
never use:

```toml
[tools]
disabled = ["map_eval", "route", "context_trim", "unity_tests"]
```

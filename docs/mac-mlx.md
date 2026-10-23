# Native Kev on Apple Silicon

The deterministic default remains `review.backend="off"`, `vision.backend="off"`.
No model is needed for CLI/MCP governance, contracts or repository checks. Build with
`cargo build --locked`; register that executable with your MCP client as usual.
Windows desktop automation and CUDA/laya checks cannot be validated on macOS.

Native MLX is an explicit alternative local transport. It uses upstream Kev directly,
without laya: `GET /v1/models` for readiness/identity and `POST /v1/systemone` for decisions.
The existing Windows/laya transport and its settings remain available and default.

## Prepare the small testing model

Inspect your machine **before loading**: `sysctl -n hw.memsize`, `sw_vers`, `df -h .`,
`memory_pressure`. Leave room for the IDE, browser and builds. The 0.8B checkpoint is a
LoRA adapter plus pointer head; it also downloads its pinned Qwen3.5-0.8B-Base weights.
Do not download 4B weights merely to test configuration. Adapter merging has a transient
memory peak; see the measured M1 results below. Avoid compiling Rust during inference
latency measurements on an 8 GiB machine.

Use Python 3.12 and uv, and a separate upstream checkout. Keep the upstream checkout,
venv and logs in a normal project directory such as `~/Projects`, outside macOS-protected
Documents/Desktop folders: launchd may be denied access to those folders even when your
interactive terminal can read them. The commands below assume Kritic is in `~/Projects/kritic`:

```sh
git clone https://github.com/jaredpalmer/kev.git ../kev
cd ../kev
git checkout 2074cc44256200bb37540103fa4572901faa10f3
uv sync --extra serve --no-dev --python 3.12
cd ../kritic
uv run --python 3.12 scripts/serve-mlx.py \
  --config examples/profiles/mac-mlx-0.8b.toml \
  --kev-dir ../kev --log-dir "$PWD/.kritic-mlx"
/bin/launchctl bootstrap "gui/$(id -u)" "$PWD/.kritic-mlx/kev-mlx.plist"
export KRITIC_CONFIG="$PWD/examples/profiles/mac-mlx-0.8b.toml"
cargo run --locked -- doctor --online --json
```

The script **only writes** an owner-readable/writable launchd plist; it executes no
programs. Inspect its fixed `ProgramArguments`, absolute venv executable and working
directory, then submit it with the explicit `launchctl bootstrap` command above.
launchd owns the detached background process, with null stdin and file logs. The job
binds only `127.0.0.1`, forces `KEV_BACKEND=mlx`, `KEV_DTYPE=bf16`, and `--device mps`,
and does not automatically restart after exit. `ProcessType=Interactive` supplies normal
foreground scheduling for inference while the process stays detached and headless;
launchd's Background resource policy can severely throttle Python/model startup. The generator refuses occupied ports
and existing plist files, and obtains physical RAM through `os.sysconf`, without a
PATH-resolved utility or shell. Its fixed `/usr/bin/env -i` argument list clears the inherited launchd environment before
setting the explicit profile variables, HOME and PATH; temperature/adapter transforms
and API keys are not inherited. `EnvironmentVariables` alone would not provide isolation.

Readiness still requires `/v1/models`; inspect the log during first download/load.
Inspect and stop **only your job** (the port determines its label):

```sh
/bin/launchctl print "gui/$(id -u)/com.kritic.kev-mlx.8009"
/bin/launchctl bootout "gui/$(id -u)/com.kritic.kev-mlx.8009"
```

Kritic never installs, starts, restarts or kills this native service, including on
timeout. Preserve unrelated launchd jobs. After your job exits, remove only its
owned plist before generating it again. The generator requires 24 GiB for larger
models as a conservative policy, not a measured hardware minimum.

## Configuration and production-size selection

[The 0.8B profile](../examples/profiles/mac-mlx-0.8b.toml) selects a pinned checkpoint,
`kev.local_runtime="mlx"`, explicit `native_url`, `native_run`, `native_base` and
`native_dtype`, and upstream alias `model="kev-latest"`. Upstream accepts `kev-latest`
and `jev-latest`; the response's model field simply echoes the request. Identity is
checked against **every** model card's run/base/backend/device/dtype. Missing fields,
torch/CPU, another model or another precision fail closed. HTTP redirects and proxies
are disabled for this loopback transport. No automatic cloud, CPU or model fallback.

The testing profile uses 600-character chunks, a 1,024 estimated-token row limit,
a 1,024 request grouping budget and a 30-second deadline. The estimate includes JSON
escaping and question text; over-budget requests are refused locally before inference.
It is conservative, not a tokenizer guarantee. Upstream separately refuses states over
65,536 tokens; the launcher disables truncation and bounds the prefix cache to one
1,024-token state. Explicit upstream truncation flags are propagated if a separately
managed service enables truncation.

To select 4B, change only `KRITIC_CONFIG` to the shipped
[4B profile](../examples/profiles/mac-mlx-4b.toml), and launch a matching service on a
sufficiently large Mac using that same config. Configuration selection/validation does
not load weights. This is a production-size template, **not production quality
certification**. Its short request budgets deliberately remain conservative. The 8 GiB
M1 was not used to load or benchmark 4B. The two profiles pin different Hub revisions.

Every MLX identity starts with `unverified-mlx:` and includes the requested model alias,
checkpoint/revision, base and dtype. It never equals the calibrated laya `kev-4b` Q8
identity, even for native 4B. All native judgments remain advisory; changing checkpoint,
precision or backend requires fresh task-specific quality checks and calibration before
introducing acceptance gates. Native model cards are self-reported runtime evidence,
not a cryptographic attestation of weights. Do not infer parity or accuracy from a
successful request. `noul` omits confidence upstream; Kritic derives binary certainty
`max(p, 1-p)` solely for compatibility, not reliability. Choice/score confidence is
validated as supplied; score distributions and legends must have the requested indices and exact textual level labels.
Native score levels in Kritic are textual; nontextual levels are refused.

## Diagnose and verify

- `config_errors`: check the annotated config and environment overrides. Native URL must
  be an HTTP loopback origin; `cpu_only=true` conflicts with MLX.
- Readiness/identity mismatch: inspect `curl http://127.0.0.1:8009/v1/models`. Correct the
  service or matching profile; do not rename an unrelated model to bypass validation.
- HTTP 422: shorten state/questions; inspect the upstream `detail` in logs. Invalid JSON,
  missing/extra answer IDs, wrong types, invalid probabilities and missing usage fail.
- Timeout: reduce the request and check memory pressure/logs. The external process stays
  running; Kritic does not terminate services by executable name for MLX.
- `status`: use `local_runtime`, `local_url`, `local_ready`, `health` and
  `gates_calibrated`. Legacy `laya_up` remains false for native MLX.
- Model-free smoke: unset `KRITIC_CONFIG`, use a fresh `KRITIC_HOME`, run `kritic --help`
  and `kritic doctor --json`, then MCP initialize and `tools/list`. Optional missing
  scanners/Windows desktop dependencies can make doctor nonzero; model-off is expected.

```sh
python -m unittest discover -s scripts -p test_serve_mlx.py
cargo fmt --check
cargo clippy --lib --bins -- -D warnings
cargo test --test kev_mlx --test external_models --test config --test unit
KRITIC_CONFIG="$PWD/examples/profiles/mac-mlx-0.8b.toml" \
  cargo test --test kev_mlx live_mlx_0_8b -- --ignored --nocapture
```

The ordinary adapter tests use loopback fixtures, not real weights. The ignored live
test requires the explicit 0.8B profile and running service, and exercises all three
question types through Kritic. It checks protocol/identity/truncation, not quality.

## Measured M1 validation

See the [exact checks, failures/skips and measurements](mac-mlx-validation.md).
The small retry-TOML `artifact_validate(use_model=true)` workflow also completed with
one chunk scored, completeness assessed, no truncation, `kev_ok=true` and `approval=null`
at the shipped 1,024 estimated-token row cap. This establishes small-review integration,
not general workload fit, model quality or parity. Larger task text/state/questions may
still exceed the conservative row cap and must be shortened.

## Upstream evidence

Protocol and loader inspected at Kev commit `2074cc44256200bb37540103fa4572901faa10f3`:
[api.py](https://github.com/jaredpalmer/kev/blob/2074cc44256200bb37540103fa4572901faa10f3/kev/api.py),
[serve.py](https://github.com/jaredpalmer/kev/blob/2074cc44256200bb37540103fa4572901faa10f3/kev/serve.py),
[checkpoint.py](https://github.com/jaredpalmer/kev/blob/2074cc44256200bb37540103fa4572901faa10f3/kev/checkpoint.py),
[0.8B model card](https://huggingface.co/jaredpalmer/kev-0.8b).
The model card distinguishes 0.8B from 4B and warns of weaker out-of-domain performance;
no published score is claimed as Kritic/M1 validation.

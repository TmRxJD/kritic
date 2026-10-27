# CLI output filter

Wrap a **noninteractive** command, archive its raw stdout/stderr, and give your agent a
bounded extractive view. Rust owns execution and process cleanup. Optional local Kev
ranks ordinary stdout records; it never runs commands, generates summaries or decides
whether the command succeeded. Filtering works without a GPU, model or MCP connection.

## Install and run

The Windows test.6 ZIP includes `bin/kev.exe` and `bin/kev-filter.exe` alongside Kritic.
The Apple Silicon `kritic-cli-macos-arm64-test.tar.gz` includes `bin/kev` and
`bin/kev-filter` plus matching source/docs. **That Mac archive contains CLI binaries
only; the Kritic MCP server still requires a source build.** Intel Mac/Linux users
can build this small package from source. These executables are not signed/notarized.
Verify the release checksums and `RELEASE-MANIFEST.json` before use.

```powershell
# From the extracted Windows package; no model or Rust installation required.
.\bin\kev.exe run --goal "Find test failures and their causes" -- cargo test
.\bin\kev.exe run --json --log-dir E:/AI/kev-runs -- cargo check
```

```sh
# From the extracted Apple Silicon package.
./bin/kev run --goal "Find test failures and their causes" -- cargo test
# Or build only the portable CLI; this does not build the Kritic MCP server.
cargo build -p kev-filter --release --bins --locked
./target/release/kev run -- cargo test
```

To install the filter alongside a source-built MCP server, use
`bash scripts/install.sh --no-harness --with-cli`. Without `--with-cli`, that
installer preserves its existing MCP-only binary selection. Use the package-only
build above when you need just the filter.

`kev` and `kev-filter` are aliases of the same implementation. No arguments shows
help; `--version` prints `kev-filter VERSION`. Neither starts an MCP server. Nothing
installs a hook, rewrites commands, downloads weights or starts a model service.
The canonical implementation is `crates/kev-filter` in this repository.

## Optional Kev ranking, including an M1

```sh
# An already running laya endpoint, using its configured Kev model.
kev run --kev --goal "Find the failure" -- cargo test

# An already running native Kev / MLX endpoint, e.g. the Mac 0.8B test profile.
kev run --kev --kev-url http://127.0.0.1:8009/v1/systemone \
  --kev-model kev-latest --goal "Find the failure" -- cargo test
```

See [Mac MLX setup](mac-mlx.md) for installing/starting the optional pinned 0.8B
profile and switching to a larger production profile. The alias `kev-latest` does
not select/download weights: the server configuration owns the actual model.
Do not infer production quality, calibration or RAM savings from a successful smoke test.

Defaults are `http://127.0.0.1:8089/v1/decide` / `kev-4b`; `/v1/systemone` defaults
to `kev-latest`. Only numeric loopback HTTP endpoints with those exact routes are
accepted. Proxies, redirects and cloud fallback are disabled. The HTTP client has
no unused TLS/default features; this is not a general cloud client. Model identity
and quality are not independently verified by the CLI, and scores are advisory.

Ranking is opt-in per invocation. Goals are capped at 200 characters; records at
500 characters. Defaults: 16 calls, a four-record queue, five seconds of cumulative
request time and one second per call. `--rank-ms` / `--request-ms` adjust deadlines
within bounded ranges. Idle command time does not consume request time. Oversized
records, queue pressure, failed/malformed responses and exhausted budgets use
deterministic selection; the report exposes ranking/fallback counts and errors.
Slow ranking never blocks the pipe-draining threads. No local model is necessary.

## What the agent receives

Repeated ordinary records, narrow progress patterns and known Cargo compile/check
chatter are suppressed deterministically. Diagnostics and their context, unknown
stderr, opening records and closing context take priority. Only short ordinary
stdout records enter optional relevance ranking.

The excerpt is bounded: **even diagnostics can exceed the budget**. Clipping is
reported, and selection never proves diagnostic completeness. Classification counts
are not net omitted-line counts: closing/diagnostic context can restore records.
A failed command remains failed regardless of the selected text.

Human output streams selected records; `--json` emits one final report. Independent
stdout/stderr pipes have no total ordering; display order can reflect asynchronous
ranking. ANSI/control characters are removed only from display. Invalid UTF-8 display
is marked lossy; raw bytes, including carriage returns, remain unchanged. Line numbers
count LF bytes; byte offsets also locate fragments of long lines.

## Retrieve the evidence

Each run prints its unique directory and saves `command.json`, `worker.json`,
`stdout.log`, `stderr.log` and final `run.json`. The root defaults to `runs/` beside
the executable. Set `KEV_LOG_DIR` or `--log-dir` to a writable, private persistent
location. The command cannot start if the archive cannot be created.

```sh
kev show /path/to/runs/run-ID --stream stderr --from 20 --lines 30
```

Retrieval is sanitized/capped display too; read raw files for exact bytes. Logs
contain command arguments/output **without automatic secret redaction or retention**.
Unix directories/files are owner-only; Windows files inherit directory ACLs. Keep
the root private and remove old runs according to your own retention policy.

## Execution and limits

Arguments after `--` are OS arguments; no shell is added. Invoke a shell explicitly
for shell syntax. Windows batch files may invoke the batch interpreter; prefer an
actual executable when literal argument semantics matter. Child stdin is closed:
run interactive prompts/TTY applications unwrapped.

| Outcome | CLI exit / evidence |
|---|---|
| Normal completion | Child exit code, including failures |
| Timeout | 124 |
| Wrapper cancellation | 130 |
| Wrapper, capture or raw-log-limit failure | 125 |
| Executor launch failure | 127 |
| Unix child signal | 128 + signal, recorded separately |

`run.json` separates child status from wrapper termination, including a child that
itself exits 124. Missing `run.json` means the wrapper did not finish reporting;
it never means success. Defaults are 600 seconds, 64 MiB combined raw logs and
8,000 excerpt characters. Adjust with `--timeout`, `--max-log-bytes` and
`--budget-chars`. The raw-log limit stops the owned command and marks capture
incomplete. Pipe failures/drain timeouts also make incompleteness explicit.

An internal executor waits until contained before starting the command. Windows
uses a kill-on-close Job Object; Unix uses an owned process group. Timeout and
cooperative cancellation target that owned scope. Deliberately escaping Unix
descendants and SIGKILL of the Unix supervisor fall outside that guarantee. No
shared model service or unrelated process is terminated.

## Validation and interpretation

```sh
cargo fmt --all -- --check
cargo clippy -p kev-filter --all-targets --locked -- -D warnings
cargo test -p kev-filter --all-targets --locked
```

Regressions run real children and check failed exit codes, raw invalid-UTF-8 bytes,
literal arguments, bounded noisy output, timeout/log limits, retrieval, typed mock
ranking and deterministic fallback. Platform-specific tests cover owned process
cleanup. Mocks prove the protocol, not model quality. Measure **all emitted output**
against raw bytes: metadata can make small commands larger, and byte reduction is
not a tokenizer/billing measurement. Inspect the raw evidence whenever the excerpt
does not establish the cause. The filter does not approve code or replace tests.

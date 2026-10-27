# Kev CLI filter: M1 validation, 2026-10-27

Scope: the standalone `kev` and `kev-filter` executables from package `kev-filter`
(version `0.5.0-test.6`). Candidate baseline:
`e3dfe14466b90c6a007868b3e35187c447c4063d`, branch `codex/kev-cli-filter`.
Mac fixes and tests are on the separate `test/m1-kev-cli-filter` branch. This is
candidate validation, not certification of a future release tag or binary.
No legacy MCP/handoff component was imported or exercised.

## Machine, model and ownership

Apple M1 Mac mini (`Macmini9,1`), 8 GiB unified RAM, native arm64 macOS 26.5.2
(25F84), Rust 1.99.1. Before work: 114 GiB free disk, memory-pressure tool reported
53% free. The checkout was clean; the prior MLX branch/history was preserved.

The existing native launchd generator started one owned job on **127.0.0.1:8009**,
with null stdin, file logs, explicit `/usr/bin/env -i` environment and MLX bf16.
No global launchd environment or unrelated job was changed. `/v1/models` independently
reported `jaredpalmer/kev-0.8b@bf75a6a8848ea6960ff2ed108d9ed44c2941174f`,
`Qwen/Qwen3.5-0.8B-Base`, `backend=mlx`, `device=mps`, `dtype=bfloat16`.
The cached upstream runtime was commit `2074cc44256200bb37540103fa4572901faa10f3`,
MLX 0.32.2, mlx-lm 0.31.3 and torch 2.8.0. No production weights were fetched/loaded.

CLI inference explicitly selected:

```sh
kev run --json --kev \
  --kev-url http://127.0.0.1:8009/v1/systemone --kev-model kev-latest \
  --goal 'Find the retry configuration mismatch' \
  --rank-ms 10000 --request-ms 2500 --log-dir ./owned-runs \
  -- /usr/bin/python3 ./controlled-fixture.py
```

This endpoint/model selection is independent of Kritic's review profile. The CLI
correctly reports `runtime_identity_verified=false`, even though the test separately
inspected the upstream model card. Its scores remain advisory; no calibration,
model quality, diagnostic-retention guarantee or parity claim follows from this test.

## Checks and recovered portability failures

- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p kev-filter --all-targets -- -D warnings`: passed.
- `cargo test -p kev-filter --all-targets --locked`: **16 passed, 0 failed, 1 ignored**.
  The ignored helper is explicitly invoked as a real subprocess by other tests.
  Windows-only Job Object/kill-on-close regression is not compiled on macOS.
- Real tests cover child exit 7, child exit 124 versus wrapper timeout, timeout of an
  actually started process, raw log limits, literal argument boundaries, invalid UTF-8
  raw bytes, UTF-8 split across reads, retrieval, unavailable-model fallback and local
  HTTP response validation/ranking. No model is required for the ordinary test suite.
- Added Unix SIGTERM regression: wait until the requested child **and its spawned
  descendant** announce startup, signal only the owned wrapper PID, require exit 130
  and `termination="cancelled"`, then confirm the descendant cannot write its delayed
  marker. This passed. It does not claim cleanup after Unix SIGKILL/crash.
- Added Unix child-signal regression: a deliberately signalled child gives wrapper
  exit 143, child `signal=15`, null child exit code and null wrapper termination;
  this is distinguished from cancellation of the wrapper.
- Strict Clippy exposed direct function-item-to-integer casts in Unix signal handler
  registration. Casting through a function pointer fixes the lint without suppressions
  or changing signal semantics.
- A subsequent test run exposed a Darwin socket portability race: a connection accepted
  by the nonblocking polling HTTP fixture could inherit `O_NONBLOCK`, causing
  `WouldBlock (os error 35)` during header reads. The accepted connection is now explicitly
  blocking with its existing one-second read timeout. The full package suite passed
  afterward. This fixes the test fixture, not the production HTTP transport.
- Actual debug CLI checks additionally passed shell metacharacters/spaces and a non-UTF-8
  argument forwarded to an explicitly selected Python child. Full raw stdout preserved
  its bytes, NUL, invalid UTF-8 and CRLF exactly; stderr was byte-exact; exit remained 7.
  `kev show --stream stderr --from 2 --lines 1` returned precisely the requested line.
  Owned run directories were mode 0700 and raw/metadata files 0600. Both CLI aliases
  reported `kev-filter 0.5.0-test.6`.

## Bounded live ranking and whole emitted-byte observations

The controlled fixture emitted three opening records, a meaningful mismatch
(`retry max_attempts is 1 but the required value is 3`), unrelated gardening and
recipe records, 5,000 identical progress records, and protected stderr diagnostics:

```text
ERROR: controlled retry validation failure
  expected max_attempts=3
  received max_attempts=1
```

The controlled `controlled-fixture.py` was:

```python
import os
print('opening build context one')
print('opening build context two')
print('opening build context three')
print('retry max_attempts is 1 but the required value is 3')
print('Garden report: the roses bloomed pink this morning.')
print('Banana bread recipe: mash ripe bananas, mix flour and bake at 175 C for 50 minutes.')
for _ in range(5000):
    print('progress status: waiting for work')
os.write(2,b'ERROR: controlled retry validation failure\n  expected max_attempts=3\n  received max_attempts=1\n')
raise SystemExit(7)
```

The child deliberately exited 7. Separate unwrapped executions produced the exact
same stdout/stderr bytes as each CLI archive. Measurements count **all emitted stdout
plus stderr**, including complete JSON and log-location metadata, not only excerpts.
All modes had complete raw capture, preserved the mismatch and protected diagnostic
text, and returned actual child/wrapper exit 7.

| Mode | Raw bytes | Complete emitted bytes | Reduction | Scored records | Model-rejected records |
|---|---:|---:|---:|---:|---:|
| Deterministic, no model | 170,363 | 3,844 | 97.744% | 0 | 0 |
| Explicit local 0.8B MLX | 170,363 | 3,848 | 97.741% | 4 | 0 |
| Requested model unavailable | 170,363 | 3,898 | 97.712% | 0 | 0 |

For the real MLX run: `kev.requested=true`, `kev.error=null`,
`kev.scored_records=4`, `kev.advisory=true`, `runtime_identity_verified=false`,
`selection.kev_rejected_candidates=0`. The relevant mismatch was retained with
reason `Kev relevance (advisory)`. **All four candidates were kept.** This demonstrates
actual inference and score-based selection, but no added pruning benefit on this
fixture. An earlier smaller fixture with housekeeping output likewise scored three
records and kept all three. Exploratory short direct requests were also bounded;
they are not CLI quality evidence. The scripted expectation of at least one rejection
was not met; no threshold was changed to force that expectation. The local typed HTTP
regression independently exercises rejection using controlled response scores.

The reduction here comes primarily from **4,999 deterministic duplicate removals**.
Do not attribute it to model quality or predict token/billing savings from these bytes.
Metadata/path lengths, OS scheduling and the fixture affect the measurements. Small
commands can produce more metadata than their original output. Excerpts remain an
incomplete projection and may clip important text; raw logs and OS outcomes remain
authoritative.

The fallback run explicitly selected `http://127.0.0.1:1/v1/systemone`, alias
`kev-latest`, request limit 50 ms and rank budget 100 ms. It reported connection
refused, zero scored records and an explicit `kev.error`, retained deterministic
output/diagnostics and preserved exit 7. No cloud, other model or CPU fallback occurred.

## Cleanup, optimized build and limits

The owned launchd model job was stopped after the bounded inference runs. Its label
was absent and port 8009 was free afterward. No full Kritic build ran concurrently
with MLX. With the model stopped and 64% free memory reported, optimized standalone
binaries were built using `CARGO_BUILD_JOBS=1 cargo build -p kev-filter --release
--bins --locked` (33.50 seconds, one build job). Both files were native Mach-O arm64.
The release CLI repeated the literal/non-UTF-8 argv, raw-byte, exit-7, permission and
retrieval checks successfully; both optimized aliases reported version test.6.
Executable code matches Mac-fix commit `e39775c`; these locally built candidates are
not published release artifacts and are not claimed to match a future release tag.

This validation does not cover interactive/TTY programs, universal Unix process
containment, Windows execution, a production model, model calibration, broad workloads
or diagnostic completeness. Parent integration/release validation owns workspace-wide
checks, packaging, final tagged-source builds and publication.

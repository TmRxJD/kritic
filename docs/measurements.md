# Measurements

Everything here was measured on Windows 11 with an RTX 5070 (12 GB), Kev-4B Q8_0 through laya
(CUDA sm_89 PTX JIT-compiled to sm_120) and `qwen2.5vl:7b` through Ollama. The raw records are in
`calibration/`. For behaviour on realistic agent changes, with and without kritic, see
[benchmarks](benchmarks.md).

## Kev critic

- **Obvious bugs in small snippets:** 4 of 4 buggy snippets were rejected and 4 of 4 fixed ones
  approved (`=` vs `===`, off-by-one, `eval`, SQL injection). Also rejected: incomplete code
  (p(complete) 0.02) and a "pre-approved" injection comment (p = 0.69).
- **Clean code:** 5 of the crate's own source files (4–12 chunks each) were approved.
- **Subtle logic bugs inside real files: weak.** 0 of 4 planted bugs were caught in files mode,
  and 1–2 of 4 in diff mode. Kev is a coarse screen; **tests, tsc and lint are the real bug
  catchers**, so pass them as `commands`.
- **Latency:** about 1.9 s per 1,300-character chunk when warm. laya falls off a cliff once a
  batch goes above about 3,700 tokens (4 rows of 942 tokens took 4.8 s; 5 rows took 19 s), so
  requests are packed to ≤ 2,900 tokens. Of 6,399 logged requests, the slowest took 5.3 s.
- **Truncation:** laya silently cuts rows at 2,048 tokens. kritic detects this from
  `usage.input_tokens` and reports `truncated_chunks`, which blocks approval.
- **Cold start:** about 40 s for the first call after laya starts (CUDA JIT); about 13 s with the
  JIT cache.

## ui_test and visual_critic

- **`ui_test`** on the harness fixtures:
  - The good app was approved: 2 pages, 7 interactions, 3 persistence checks held.
  - The broken app was rejected with every planted defect found: a throwing handler, a
    `console.error` handler, a no-effect button, a 404 fetch, a missing image, a non-persisting
    input, and the `image-alt` and `color-contrast` axe violations.
  - Correctly skipped: "Delete account" (looks destructive) and an external link.
  - About 5–10 s per page set.
- **`visual_critic`:**
  - A polished page scored 9/9/9/9/8 and was approved. A messy page scored 0 across the board and was rejected. A plain unstyled page scored 5–6, rejected at the default threshold of 6.
  - The model sometimes invents issues: it reported "Email field text color is too light" on the
    polished page, which isn't true. Hence `unverified_observations`; use axe for verified contrast.
  - 1.3 s per image once warm with laya stopped. The first call after a GPU hand-off also loads
    the model.
- **GPU sharing:** with Kev and the vision model both resident beside a desktop on the 12 GB card,
  Windows spilled VRAM into system RAM and a vision call went from 1.3 s to 66 s. Hence one model on
  the GPU at a time.

## security_critic, code_quality_critic, resource_critic, desktop_test

- **`security_critic`**, on a vulnerable sample generated at test time:
  - Rejected with score 0: gitleaks found the AWS key; osv-scanner found 3 vulnerable packages at
    high; bandit found `shell=True`, `eval` and SQL string building; semgrep found `shell=True`. About 7 s.
  - Kev's triage ordering was sensible: `shell=True` 0.87–0.90, the AWS key 0.84, SQL 0.78, `eval` 0.52,
    the low-value "import subprocess" warning 0.35.
  - The secret was never echoed. A clean project was approved with score 10.
- **`code_quality_critic`** on this crate: 131 functions, clippy clean, 4 ruff findings in the
  Python harness. It flagged one function at complexity 26 > 20; after a refactor the crate passes
  at ≤ 18. Kev's maintainability judgment is fairly flat (0.48–0.73 for every hotspot): a weak signal.
- **`desktop_test`** on the WinForms fixture: good mode approved. Broken mode rejected with every
  planted defect found: an error dialog with its text, a no-effect button, a crash (exit code 3), a
  field that doesn't persist across a relaunch, and a stderr error. "Delete all data" was skipped.
  Hang mode reported a 12 s UI freeze. About 50 s per run.
- **`resource_critic`:** a process leaking 1,120 MB/min was rejected by the deterministic budget and
  an idle one approved. **Kev's own resource judgment is unreliable**: it called the leak "leak risk
  low" (p = 0.64–0.72) and "reasonable" (0.91). Approval rests on the measured budgets.

## A real Unity application

kritic was battle-tested on a private Unity desktop application that publishes its state on a
WebSocket:

- **App event stream:** `desktop_test` with `app_events` got readiness pushed 6.9 s after launch.
  9 socket steps (open a dialog, assert its controls, screenshot, close, a planted missing control)
  took 90–590 ms each. The planted failure was reported with the app's own message. 22 s in total
  including launch.
- **Found and fixed in kritic:** harness input decoded as cp1252 (`Item Lyrics…` arrived as
  `Item Lyricsâ€¦`; stdin is now read as UTF-8); runs that stalled forever on an app that never
  signalled ready (now `stall_s` fails fast); ports left bound by an exited process's inherited
  handle (now reported by `hygiene_critic`).
- **Domain checks as plugins:** a repo plugin scored generated content against a ranked reference
  corpus in code (ranked 10/10, random 0/10), which Kev could not do. That is the pattern
  [plugins](plugins.md) describe.

## ux_critic

See [ux_critic → calibration](ux-critic.md#calibration).

## Context use

Before compact outputs were added, one long build session reached 800k context tokens (80% of 1M)
with 0 compactions and 130.7M processed input tokens over 285 calls (99% cache reads). That is why
every large report now goes to `full_result` and the response keeps only problems.

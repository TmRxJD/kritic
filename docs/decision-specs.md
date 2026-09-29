# Decision specs: how Kev is asked

## Kev in one paragraph

**Jev** is TypeSafe AI's evaluation model: instead of writing text, it reads a small declared state
and answers typed questions (yes/no, a choice from a declared set, or a score on a declared scale),
each as a probability. Its weights are closed. [Kev](https://github.com/jaredpalmer/kev) is an open,
Apache-2.0 family of Jev-style typed-decision models; the "k" in kritic is for Kev. Kev-4B is built
on Qwen and served locally by [laya](https://github.com/monatis/ggmlc/tree/main/examples/laya) from a
GGUF. There is no free text to parse, and no explanation to trust or distrust.

That shape is why kritic uses it: a probability can be thresholded, calibrated and audited, and a
typed answer can't invent a finding. Kev is also only as good as the question it is given, so
nothing in kritic writes a question inline.

## Specs

Every Kev call goes through a **decision spec** (`specs/*.json`, schema in `src/spec.rs`, checked
by the `spec_lint` tool):

- **State fields** are declared: a name, a kind and a one-sentence meaning. The kinds:
  - `text`;
  - `code`, with a size limit (callers chunk; nothing is truncated);
  - `band`: a value that code has already measured and judged, written as a term from a declared
    vocabulary (TYPICAL, HIGH, BEYOND, FAR_BEYOND…).

  Kev never sees raw numbers. Measured, it read 0.794 against a ranked maximum of 0.009 as "in range".
- **Questions** name the fields they read in backticks. A yes/no question holds one condition and
  defines both answers; every choice option and score rung is defined. A question reads at most
  two fields (yes/no) or three (choice/score): combining many fields is code's job. Measured, a
  five-field question misjudged an obviously broken input.
- **Gates.** A question can fail approval only with dated calibration evidence and its good-case
  tail. Everything else is advisory, with the measured reason.
- **Enforced.** The built-in specs are linted in tests and at startup. Plugin `[kev]` blocks are
  linted on discovery. `spec.state()` rejects missing, extra, oversized or off-vocabulary values. The
  `Question` type has no public constructor outside the spec layer.

## The shipped specs

| Spec | Used by | Units |
|---|---|---|
| `code_chunk` | `critic_score` (files mode) | ~1,300-character chunks |
| `code_change` | `critic_score` (diff mode) | diff hunks |
| `completeness`, `completeness_digest` | `critic_score` | once per call: the change against the request |
| `security_triage` | `security_critic` | one finding with ±6 lines |
| `function_maintainability` | `code_quality_critic` | one complex function |
| `resource_fit` | `resource_critic` | measured budgets as bands |
| `route` | `route` | the task description |
| `context_relevance` | `context_trim` | one context chunk against the task |
| `ux_novice_step` | `ux_critic` (novice fallback) | one screen's numbered controls |
| `map_slice` | `map_eval` | a 16-beat stretch |

`status().gates` lists every question that can currently fail approval, with its rule.

## Calibration

`scripts/calibrate-specs.mjs` asks laya directly, with each spec's exact state and questions and
no kritic code in between, over labelled cases:

- clean code;
- safe decoys: comments that describe bugs or reviews, and safe subprocess/socket/env code;
- the same code with a planted problem: one mutated operator, an inserted vulnerability, a stub,
  or a reviewer-directed comment;
- hand-written full and partial implementations of a request.

A question may gate only if its AUC is at least 0.85, and one threshold keeps false rejections
at or below 5% while catching at least 50%. `scripts/write-specs.py` writes the gates from the
calibration output, so no threshold is typed by hand.

Approval needs every unit to pass, so a flat per-unit threshold compounds: a clean 29-hunk change
was falsely rejected, since 1 − 0.95²⁹ = 77%. Each gate therefore carries its good-case tail and
is applied at a per-unit false-reject rate of 0.05 / n.

The shipped gates were calibrated on 2026-09-27 on this repository plus a private
TypeScript/Python application (`calibration/specs-2026-09-27.json`). To recalibrate on your own code:

```bash
node scripts/calibrate-specs.mjs --corpus ../my-app/src --corpus ../my-app/scripts --history ../my-app
python scripts/write-specs.py
```

## What calibration showed

- **Kev cannot find subtle logic bugs.** One mutated operator scored AUC ~0.5 on both chunks and
  hunks. `defect` and `severity` are advisory; the project's tests, type checks and linters catch bugs.
- **It is good at** unfinished work, reviewer-directed text and completeness (on code), and useful
  on security in diffs.
- **The request wording biased it.** The same diff scored p(severe) 0.67 under a request that
  talked about fixing things and 0.36 under a neutral one. Unit states therefore never contain the
  request; only the completeness spec does.
- **On numbers it is unreliable.** It called a process leaking 1.1 GB/min "leak risk low", so
  `resource_critic` approves on measured budgets. On Beat Saber map statistics it didn't separate
  random notes from ranked maps, so `map_eval` is deterministic.

See [measurements](measurements.md) for the full record and [benchmarks](benchmarks.md) for
behaviour on realistic changes.

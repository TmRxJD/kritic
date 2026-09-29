# Repo plugins

A project adds its own evaluators without changing kritic. Put a manifest in
`<repo>/.kritic/plugins/`: the agent finds it with `plugin_list(base_dir)` and runs it with
`plugin_run(base_dir, name, args)`. A worked example is in
[`examples/plugin-repo`](../examples/plugin-repo).

Use a plugin for anything specific to one project or domain: scoring generated content against a
reference corpus, checking a file format, enforcing a house rule. Change kritic itself only for
capabilities every project needs ([extending](extending.md)).

## Manifest: `.kritic/plugins/<name>.toml`

```toml
name = "map-score"                        # letters, digits, _ or -
description = "Score a generated level for flow, balance and density"
command = ["node", "tools/kritic/map-score.mjs"]  # relative paths resolve against the repo; {base_dir} and {out_dir} are substituted
timeout_s = 120
findings_advisory = false                 # true: high/critical findings don't fail approval

[input_schema]                            # documents args; `required` keys are enforced
type = "object"
required = ["map"]
properties.map = { type = "string", description = "path to the level file" }
```

JSON manifests (`<name>.json`) with the same keys work too.

## Contract

- **stdin:** `{"args": {...}, "base_dir": "...", "out_dir": "..."}`. `out_dir` is a scratch folder for artifacts.
- **stdout:** exactly one JSON object:

  ```json
  {"ok": true, "approval": true, "summary": "...", "error": "...",
   "findings": [{"severity": "critical|high|medium|low|info", "message": "...", "file": "...", "line": 12}],
   "metrics": {}, "kev_state": {"<field>": "<value>"}}
  ```

  Only `ok` is required. `kev_state` is required when the manifest has a `[kev]` block (below).
- **stderr:** optional progress, one JSON object per line with an `"event"` key. These stream live
  to kritic's WebSocket. Anything else on stderr is kept as error context.

## How kritic judges a plugin

approval = the plugin's `approval` (if given) AND no high/critical findings (unless
`findings_advisory`) AND every calibrated Kev gate passes. Advisory Kev questions are only reported.

`ok: false`, bad JSON, a timeout or a missing required argument gives `approval: null` with the
error. A result is never faked. The full output is saved as `full_result`; responses list at most
25 findings.

## Optional: Kev questions

Only add Kev when it must judge something code cannot decide. If code can band a number, code
decides and Kev is not asked. The `[kev]` block is a full [decision spec](decision-specs.md):

```toml
[kev]
name = "map_score_kev"
purpose = "Judge whether the stretch the plugin quotes reads as natural movement."

[[kev.fields]]                            # every field Kev sees: name, kind, one-sentence meaning
name = "stretch"
meaning = "The 16 steps the plugin judged densest, one per line: time, hand, column, row, direction."
kind = { kind = "code", max_chars = 1400 }

[[kev.questions]]                         # names its fields in backticks; defines every answer
id = "natural"
ask = { type = "noul", instructions = "Do the movements in `stretch` flow naturally?", yes = "each hand moves in smooth, repeated arcs", no = "the hands jerk between unrelated positions" }
gate = { gate = "advisory", why = "not calibrated yet: needs labelled stretches before it may gate" }
# A gate that fails approval needs dated calibration evidence and the good-case tail:
# gate = { gate = "rule", measure = "yes", fails_when = "below", threshold = 0.4, reason = "...",
#          calibration = "2026-09-27: 40 reference vs 40 generated stretches, AUC 0.91, 5% false rejects, 80% caught",
#          good_tail = [[0.05, 0.4], [0.02, 0.33], [0.01, 0.3], [0.005, 0.28], [0.0, 0.25]] }
```

The plugin returns `kev_state` with exactly those fields, each within its limits; band values start
with a vocabulary term. A mismatch makes approval `null`.

The block is linted when the manifest is discovered. `plugin_list` reports the manifest as an error
instead of running it when there is:

- a vague question;
- a field no question reads;
- a yes/no question with "or" in it;
- an undefined answer;
- a gate without dated calibration evidence.

Check a block with `spec_lint` before shipping it.

## Rules for plugin authors

1. **Compute, don't guess.** Findings and metrics come from real analysis: parsing, validation,
   measurement against a reference. Kev only judges your numbers; it never invents findings.
2. **Be deterministic.** The same input gives the same output. Put thresholds in the manifest or args.
3. **Keep output small.** Write big artifacts to `out_dir` and reference them in `metrics`.
4. **Ask Kev only about what your metrics show.** Kev sees `kev_state`, not your input files.

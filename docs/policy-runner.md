# Policy runner and trajectory diagnostics

A game-agnostic frame loop plus trajectory diagnostics. The game supplies a **schema** (what its
bytes mean, numerically) and a **client** that speaks a small framed protocol; kritic supplies the
loop, timing, recording, diagnostics and the fail-fast veto. kritic never interprets a field: it
decodes numbers and applies the thresholds the game declares. Everything runs on this host
(loopback TCP or a child process's stdin/stdout); there are no network calls.

Tools: [`policy_schema`](#policy_schema), [`policy_run`](#policy_run),
[`trajectory_diagnose`](#trajectory_diagnose). Code: `src/policy/`. Tests: `tests/policy.rs`
(a synthetic toy client with no meaning).

## The loop

Per tick:

```
state_t  = capture_generic_game_state_buffer()            # STATE frame from the client
action_t = evaluate_policy(state_t, game_provided_rules)   # a pluggable Policy
send_generic_action_buffer_to_game(action_t)               # ACTION frame
log_generic_frame_telemetry(state_t)                       # in memory, written per episode
```

Two modes:

| mode | the client | reproducible | use |
|---|---|---|---|
| `lockstep` (default) | waits for each ACTION before stepping | yes: equal seeds give equal `digest`s with a deterministic client and policy | regression sweeps, diagnostics, slow policies (game time simply waits) |
| `hold` | is answered at once with the last finished action while the policy evaluates a snapshot in the background | no (depends on wall-clock timing) | keeping the native tick rate with a policy slower than one tick |

`lockstep` is effectively time dilation: the game's simulated time advances one fixed tick per
STATE, however long the policy takes. The report still measures policy time against the native
budget (`1/tick_hz`), so a policy that cannot keep up is reported, not hidden.

### Two-layer control (reflex + intent)

A slow evaluator should not drive per-tick values. Split control:

- **Reflex layer (game side, every tick):** the client's own fast controller turns the current
  intent into per-tick values. It runs inside the game loop with no transport or model latency.
- **Intent layer (policy side, low rate):** the action heads describe a *mode* or *target*, not a
  per-tick value. The policy chooses it every N ticks (`kev.every_ticks`) or whenever an
  evaluation finishes (`hold`), and the choice is latched until the next one.

The protocol needs nothing extra for this: the schema's action heads are whatever the game
decides they mean, and an empty ACTION tells the client to use its own policy for that tick.

## Schema

JSON. `policy_schema` with no input returns its JSON Schema. Example (meaningless toy):

```json
{
  "name": "toy",
  "version": "1",
  "tick_hz": 60,
  "state": {
    "endian": "little",
    "fields": [
      {"name": "x", "dtype": "f32", "count": 2, "min": -10, "max": 10},
      {"name": "p", "dtype": "f32", "min": 0, "max": 100},
      {"name": "k", "dtype": "u16", "min": 0, "max": 1000}
    ]
  },
  "action": {
    "packing": "byte_per_head",
    "heads": [{"name": "h0", "values": 3}, {"name": "h1", "values": 2, "labels": ["off", "on"]}]
  },
  "thresholds": {
    "progress_field": "p",
    "success_at": 50,
    "watch_fields": ["x[0]", "x[1]", "p"],
    "stuck_ticks": 10, "stuck_epsilon": 0.001,
    "oscillation_window": 10, "oscillation_flips": 4, "oscillation_amplitude": 0.04,
    "unrecoverable": [{"field": "x[1]", "below": -5, "for_ticks": 5}],
    "min_family_success": 0.0,
    "cluster_radius": 0.1
  }
}
```

- **State** (`state_t`): fields packed in order, no padding. `dtype`: `u8 i8 u16 i16 u32 i32 f32
  f64`; `count` > 1 makes an array, addressed as `name[i]`. `min`/`max` set the normalization
  range for diagnostics (the observed range is used when absent). `endian`: `little` (default)
  or `big`.
- **Action** (`action_t`): discrete heads, each with `values` choices. Packing:
  `byte_per_head` (one byte per head, at most 256 values) or `mixed_radix_u32` (one little-endian
  u32: `a0 + n0*(a1 + n1*(a2 + ...))`). Optional `labels` are opaque text for policies that read
  text (Kev).
- **Thresholds**: see [Diagnostics](#diagnostics). Field references are checked at parse time,
  and every problem is reported at once.

## Protocol

Little-endian throughout. Every frame:

```
u32 length   # bytes after this field (kind + payload), 1..16 MiB
u8  kind
... payload
```

| kind | name | direction | payload |
|---|---|---|---|
| `0x01` | HELLO | client -> runner, once, first | UTF-8 JSON `{"schema": name, "version": v, "state_bytes": n}`; a mismatching `schema` or `state_bytes` is refused |
| `0x02` | RESET | runner -> client | `u64 seed`, `u32 max_ticks`: start an episode, then send its first STATE |
| `0x03` | STATE | client -> runner | `u64 tick`, `u8 flags` (bit 0 = terminal), then exactly `state_bytes` of `state_t` |
| `0x04` | ACTION | runner -> client | `u64 tick` (the STATE it answers), then `action_bytes` of `action_t`, or nothing = use your own policy this tick |
| `0x05` | BYE | runner -> client | none: no more episodes, close |
| `0x7F` | ERROR | either | UTF-8 message; fatal |

Client sequence:

```
send HELLO
loop:
  read frame
  RESET  -> reset to seed, send STATE(tick 0)
  ACTION -> apply (empty = own policy), advance one tick, send STATE
  BYE / ERROR / EOF -> exit
```

An episode ends after a terminal STATE or after `max_ticks` STATEs; the runner then sends the
next RESET or BYE instead of an ACTION. Transports: `connect` to a client listening on a loopback
address (anything else is refused), or `command` to spawn the client (no console window) and use
its stdin/stdout. A minimal client is about 40 lines: see `toy_client` in `tests/policy.rs`.

## Policies

`policy` is `{"kind": ...}`:

| kind | what | notes |
|---|---|---|
| `defer` (default) | empty ACTIONs: the game's own policy acts | kritic only records and diagnoses |
| `constant` | `{"action": [..]}` every tick | |
| `cycle` | `{"actions": [[..], ..], "hold_ticks": n}` | |
| `rules` | `{"rules": [{"field", "below"?, "above"?, "action"}], "default": [..]}`, first match wins | |
| `random` | uniform per head, generator seeded by the episode seed | reproducible |
| `process` | `{"command": [..], "cwd"?}`: an external local policy process | gets RESET / STATE frames on stdin, answers ACTION frames on stdout, gets BYE |
| `kev` | `{"every_ticks": 30, "instructions"?}`: one Kev `choice` question per head, held between decisions | see the limits below |

The `Policy` trait (`src/policy/policies.rs`) is the extension point; implement `begin`, `act`
and optionally `end` / `stats`.

### Kev as a policy: limits

Measured on this host (RTX 2070, laya `/v1/decide`, warm, 4-value choice, 132 input tokens):
**~105-130 ms per decision**, about 8 Hz. A 60 Hz tick is 16.7 ms, so Kev cannot evaluate every
tick at native rates. The runner reports this as `timing.realtime_feasible = false` with the
measured percentiles.

Same model and laya build on the RTX 5070 (benchmarked 2026-10-19, same requests alternated
between the two cards, median client latency; choices identical, probabilities within 0.005):

| request | input tokens | RTX 2070 | RTX 5070 | speed-up |
|---|---|---|---|---|
| 4 fields, 1 head | 194 | 126 ms | 61 ms | 2.1x |
| 32 fields, 1 head | 675 | 338 ms | 141 ms | 2.4x |
| 128 fields, 1 head | 2048 (window full) | 935 ms | 375 ms | 2.5x |
| 32 fields, 4 heads | 2700 | 1314 ms | 506 ms | 2.6x |

Even the 5070 is about 16 Hz on the smallest state, short of a 60 Hz tick, and latency grows
with the number of fields sent and heads asked, so send only the fields the decision needs.

- laya already answers a typed question in one forward pass with no generated tokens (the
  response reports `output_tokens: 0`); latency is prefill of the state text, so it scales with
  how many fields are sent, not with output length.
- Kev is a typed-decision classifier trained for yes/no and pick-one judgments about text, not a
  control policy. Over unexplained numeric state its choices are close to uniform (the test call
  above returned probabilities 0.18-0.31 over 4 options). Use it for smoke runs, low-rate intent
  probing, or offline triage of recorded trajectories, not as a competent agent.
- Use `every_ticks` in `lockstep` (game time waits during each decision) or `hold` (native rate;
  `timing.mean_action_age_ticks` reports how stale applied actions were). For per-tick control,
  plug in a scripted or `process` policy and keep Kev for intent or triage.
- A `kev` run holds kritic's Kev GPU lock for its whole duration.

## Diagnostics

Computed in code from the schema's thresholds; no model is involved.

- **Signature** per trajectory, per dimension, normalized to the dimension's range: mean,
  standard deviation, and the mean over each quarter of the episode (distribution over time).
- **Behavior families**: deterministic leader clustering on signatures (RMS distance <=
  `cluster_radius`), then one reassignment pass. Families are numbered by first appearance.
- **Stuck**: a stretch of at least `stuck_ticks` where every watched field stays within
  `stuck_epsilon` of its range.
- **Oscillation**: within `oscillation_window` ticks, at least `oscillation_flips` direction
  reversals by steps of at least `oscillation_amplitude` of the range.
- **Unrecoverable**: a metric past `below`/`above` from some tick to the end of the episode, for
  at least `for_ticks`.
- **Progress / success**: progress is made when `progress_field` rises more than
  `progress_epsilon` above its first value; success when it reaches `success_at`.
- **Veto**: every family whose success rate is at or below `min_family_success` (default 0, i.e.
  0% success) is rejected. `veto.families[]` carries the family's seeds, `reason`
  (`no_progress`, `no_success`, `below_min_success`), its coordinate signature (`coordinates`
  with `coordinate_fields`, plus the same in the game's units as `signature`) and its dominant
  issues. `approval` is `false` when any family is vetoed. The building agent should treat this
  as a structured rejection: revise the generated content or policy, and use the signature to
  see which fields the dead family sits on.

## Recording

`policy_run` writes `episode-<seed>.jsonl` under `<server.runs_dir>/policy/<schema>-<time>/`
(`record: false` to skip): a header line `{"seed", "schema", "version", "policy", "mode",
"terminal", "digest"}` then one line per tick `{"t", "s": [..], "a": [..] | null, "us"}`.
`trajectory_diagnose` reads a file or a directory of them. JSON keeps each value to within one
ulp, so re-diagnosing a recording matches the live run's diagnostics but not bit-for-bit digests.

## Tools

### policy_schema

`{path?, content?}` -> `ok`, `errors[]`, dims, byte sizes, flattened field labels, action heads,
per-tick budget; with neither input, `definition_schema`.

### policy_run

`{schema_path | schema, connect | command (+cwd), policy, rules, seeds | episodes, max_ticks
(1000), mode (lockstep), frame_timeout_ms (5000), record (true)}` -> `approval`, `timing`
(`policy_us_p50/p95/max`, `budget_us`, `deadline_misses`, `achieved_hz`, `realtime_feasible`,
`held_ticks`, `mean_action_age_ticks`, `reproducible`), `episodes[]` (`seed`, `ticks`,
`terminal`, `digest`), `diagnostics`, `policy_stats`, `record_dir`, and `notes` with any
feasibility limits.

### trajectory_diagnose

`{schema_path | schema, path | trajectories}` -> `approval`, `veto`, `families`, `episodes`,
`ranges`.

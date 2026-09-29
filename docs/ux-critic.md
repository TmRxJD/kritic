# ux_critic: UIs judged by workflow

`ux_critic` asks whether a user can find each mode and core action, and how many steps each
takes. It is built to be hard to game: approval comes from **real task completion against
deterministic done-checks** the repo declares, never from a model's opinion of the layout.

## The spec: `<repo>/.kritic/ux.toml`

Examples: [`examples/ux/web.ux.toml`](../examples/ux/web.ux.toml),
[`examples/ux/unity.ux.toml`](../examples/ux/unity.ux.toml). The spec is validated with every
error listed at once: unknown keys, bad regexes, a predicate the app kind can't evaluate, and a
primary budget over 3 are all errors.

```toml
ignore = ["^Quit$"]                 # label regexes never pressed (still audited by the rules)

[app]
kind = "web"                        # web | desktop | unity
purpose = "A notes app: browse notebooks, write notes, share a note."
url = "http://localhost:5173/"      # web (a path without a scheme is a file beside the spec)
# desktop / unity: exe, args, cwd, env, window_title_re
# unity: app_events, ready_event, ready_timeout_s, shot_dir
# settle_ms (web, default 150), viewport = [1280, 800]

[[modes]]                           # each mode needs a top-level entry: done within 1 action of home
name = "Editor"
done = { control = "^Save note$" }

[[tasks]]
name = "share"
goal = "Share the note Weekly plan with a teammate"   # plain words: all the novice gets
done = { text = "Share with" }
budget = 3                          # default 3; primary tasks may not exceed 3
primary = true
```

`done` holds when every predicate given holds on an observed screen:

- `control`: a visible control's label matches;
- `text`: the screen's visible text matches;
- `url`: web only;
- `event`: an app event since the last reset matches (needs `app_events`).

A task whose done-check already holds on the home screen is a hard fail: it measures nothing.

## Adapters: one contract, three kinds

A persistent harness process per run speaks JSON lines: `start`, `reset` (fresh start state),
`observe` → `{fingerprint, screenshot, controls: [{id, label, text, tooltip, role, rect, enabled,
selected, skip}], text, events}`, `act(id)`, and `close`. Every call is bound by `stall_s`
(default 15 s): a stall ends the run with an error instead of waiting. The process tree is killed
if it doesn't close, and desktop/Unity runs report what they left alive (`hygiene`).

- **web** (`harness/ux_web.mjs`): Playwright. Reset is a fresh browser context (no storage). The
  fingerprint is the path and hash, the controls and their state, and the visible text with digits
  folded (a ticking counter is not a new screen).
- **desktop** (`harness/py/ux_adapter.py`): UI Automation; reset relaunches. Tooltips are UIA HelpText.
- **unity** (same file): the app's own harness commands over its event socket (`controls`,
  `click <path>`, optional `home` and `shot`); reset uses `home`, else relaunches. See [Unity](unity.md#ux_critic-on-unity).

## Engine

1. **Rule audit** of every observed screen:
   - an enabled control with no label (error);
   - an icon-only control with no tooltip;
   - a target under 24×24 px (inline links exempt);
   - axe `color-contrast` on web (error);
   - a disabled control with no tooltip saying why.

   Error-level rules are hard fails.
2. **Exploration**: breadth-first over screens (fingerprint → action → fingerprint).
   - It tries every actionable control from each screen, from a fresh reset plus a replay of the
     path, to `max_depth` (default: the largest budget + 2, at most 6) and `max_actions` (150).
   - It gives each mode and task its `optimal_steps` and path.
   - Hard fails: a mode not reachable in ≤ 1 action, and a task that is unreachable or over its budget.
   - `mean_step_distance` (the distance between successive controls of the optimal path, as a
     fraction of the screen diagonal) above 1/3 is the `scattered` finding: the workflow's controls
     don't sit in one panel.
3. **Novice**: a deliberately weak navigator.
   - It sees only the goal, what it has already pressed, a set-of-marks screenshot and the numbered
     labels (`(no label)` for unlabelled controls). It picks one control per step, for up to
     budget + 3 steps from reset.
   - It runs on the vision model at temperature 0.7 with seeds 1–3, under the GPU lock.
   - It is reported per task: successes, `within_budget`, `mean_steps` and the controls tried.
   - It fails a **primary** task only when it fails at least 2 of 3 runs.
   - Without Ollama, Kev answers through the spec `ux_novice_step`. Calibration made that fallback
     advisory (below).
4. **Report**: `approval` is false on any hard fail, and null if the run could not finish or a
   requested novice could not run. `result.json` sits beside `screen-NNN.png` and `novice/*.png`.
   `critic_brief` gives the `ux-critic` agent ([`examples/claude/agents/ux-critic.md`](../examples/claude/agents/ux-critic.md))
   the purpose, modes, tasks, report path, screenshots and a ready prompt.

## Calibration

Four contrastive pairs build the same app well and badly (`harness/test/fixtures/ux/`, `tests/ux.rs`):

- `modes`: a mode buried in another mode's menu, against a top-level tab;
- `labels`: an icon-only unlabelled toolbar, against a labelled one;
- `depth`: the primary action five clicks deep, against one;
- `group`: a workflow's controls scattered to the corners, against grouped.

The definitions were written from principles, not tuned to the fixtures: WCAG 2.5.8 target size,
one action for a mode entry, and a third of the screen for a panel.

Measured 2026-09-28 (`calibration/ux-2026-09-28-*.json`). Exploration took 27–125 actions per
variant; a whole variant with the vision novice took 39–207 s.

| Pair | Deterministic: good | Deterministic: bad | Vision novice, good / bad (3 seeds per task) |
|---|---|---|---|
| modes | every mode ≤ 1 action, every task within budget; approved | a mode 3 actions away (hard fail); a task 4 > 3 | 12/12 / 8/12 |
| labels | 0 rule findings; approved | 9 `unlabelled` (error), 9 `icon_no_tooltip` | 21/21, all in 1 step / 20/21, 17 within budget: universal glyphs (play, export, undo, gear) found first try; domain actions took 1–5 steps |
| depth | Export 1 action; approved | Export 5 actions > 3 (hard fail) | 3/3 in 1 step / 2/3, none within budget |
| group | mean step distance 0.07; approved | 0.68 of the diagonal: `scattered` (warn) | 3/3 / 3/3: the novice does not see scattering; only the geometric rule does |

The rules and `optimal_steps` separated every pair deterministically. The vision novice separated
buried modes and deep actions and never failed a good variant, but it could not see grouping or
recognisable icons. Two fixes came out of calibration. The novice now remembers what it pressed:
without that, it pressed the same button six times on a good panel and failed. And Ollama's server is
started directly: a tray launcher gave up during a 24 s GPU discovery.

The **Kev text novice** (fallback) scored modes 9/12 vs 5/12, depth 2/3 vs 0/3, labels 15/21 vs
13/21 and group 1/3 vs 1/3, and failed good-variant primary tasks 2 of 3 times. It is therefore
**advisory**: reported, never a hard fail.

## Limits

- Exploration replays from a fresh reset for every action, so the app must be deterministic from
  its start state (divergence is listed in `exploration.problems`). Cost: about 0.5–1 s per action
  on web, a relaunch per action on desktop. Keep `max_actions` modest and use `ignore`.
- `ux_critic` only knows what the spec declares. A task list that omits the real workflows
  measures nothing, and changing budgets or `done` checks to pass is gaming it.

# Unity applications

Unity draws its own UI, so Windows UI Automation sees no controls. kritic tests Unity apps in
three ways:

- **`desktop_test` Unity mode**: launch, readiness, scripted steps, soak, crash and hang detection,
  Player.log analysis, screenshots and optional frame timing;
- **`unity_tests`**: the project's Unity Test Framework tests, headless;
- **`ux_critic` with `kind = "unity"`**: workflow review, when the app answers harness commands.

## desktop_test: Unity mode

Unity mode switches on automatically when `UnityPlayer.dll` sits beside the exe. In this mode kritic:

- launches the player windowed with `-logFile` and any `env` you pass;
- waits for readiness, from `ready_event` over the app's WebSocket (`app_events`), else `ready_log`
  (a Player.log regex), else `ready_title`;
- runs `unity_steps`;
- soaks for `soak_s`, watching for an exit, crash-handler windows and hangs;
- takes `PrintWindow` screenshots, analyses Player.log (exceptions, errors) and, with
  `frame_timing=true`, measures per-frame timing with PresentMon.

Steps:

| Step | Meaning |
|---|---|
| `{do: "key", value: "space"}` | posted key press |
| `{do: "click", target: {x: 0..1, y: 0..1}}` | posted click at a window-relative point |
| `{do: "wait", value: ms}` | fixed wait: prefer `await_event` |
| `{do: "send", value: "<command>", until?: regex, timeout_ms?}` | send a command over `app_events` and wait for its reply event; a reply with `"outcome":"err"` fails the step |
| `{do: "await_event", value: regex, timeout_ms?}` | wait for an app event |

`stall_s` (default 15) fails fast: no event, no log growth and no socket connection for that long
ends the run. `fail_log` is a regex that fails the run as soon as Player.log matches it. If the
`app_events` port is already held when the run starts, the app is not launched and the holder is reported.

### The app event socket

An app that publishes its own state gives kritic push-based readiness instead of sleeps and
polling. The reference protocol:

- **Events** are JSON objects with an `event` key, for example
  `{"event":"workspace-ready"}`, `{"event":"ui-settled"}` or `{"event":"scene-loaded","scene":"Main"}`.
  Every event is relayed live to kritic's own stream.
- **Commands** are text lines `id|verb|argument`. The app answers each with an event carrying the
  same `id`, an `outcome` (`ok` or `err`) and a `detail`:
  `{"event":"command","id":"3","outcome":"err","detail":"no control named Lyrics Status"}`.
- Useful verbs: `click <name>`, `assert <name>` (fails when the control is missing; a `find` that
  answers ok on no match is useless for testing), `type`, `scroll`, `shot <name>`.

A complete call:

```json
{"exe": "C:/Builds/MyApp/MyApp.exe",
 "args": ["--workspace", "kritic-test"],
 "env": {"MYAPP_HARNESS": "1"},
 "app_events": "ws://127.0.0.1:4733", "ready_event": "\"event\":\"workspace-ready\"",
 "soak_s": 5,
 "unity_steps": [
  {"do": "send", "value": "1|click|Menu Analysis"},
  {"do": "await_event", "value": "\"event\":\"ui-settled\""},
  {"do": "send", "value": "2|assert|Analysis Panel"},
  {"do": "send", "value": "3|click|Analysis Close"}]}
```

- Run against a separate workspace or profile, so the user's own session is untouched, and clean
  it up afterwards.
- Don't send a quit command: kritic closes the app itself, and an exit during the soak counts as a crash.

### Frame timing

`frame_timing=true` uses [PresentMon](https://github.com/GameTechDev/PresentMon), and `min_avg_fps`
sets a budget. PresentMon needs an elevated process, or membership in the local **Performance Log
Users** group followed by a sign-out and sign-in. Without either, frame timing is reported as
unavailable; it is never guessed.

## unity_tests

```text
unity_tests(project_path="<Unity project>", platform="EditMode" | "PlayMode")
```

It runs `-batchmode -runTests` with the editor version from `ProjectSettings/ProjectVersion.txt`,
found under `unity.hub_editors`; `unity.editor` forces one editor. NUnit XML is parsed. Any failed
test means `false`; nothing ran means `null`. The project must not be open in another editor.

## ux_critic on Unity

The adapter asks the app itself. Its harness must answer, over the event socket:

- `controls <label>`: write `{"screen": [w, h], "controls": [{path, label, text, tooltip, role,
  rect, enabled, selected}], "texts": [...]}` to a JSON file and reply with its absolute path.
  - Controls are every active, on-screen, uncovered `Selectable`.
  - `rect` is x, y, w, h in window pixels, with the origin at the top left.
  - `label` is the visible text, else the tooltip.
  - `texts` are the visible `Text`s outside controls (headings, status lines), which `text`
    done-checks read. An entry may be `{text, rect}`.
- `click <path>`: press the control at that hierarchy path (paths are the control ids).
- `home` (optional, strongly recommended): close menus and dialogs, stop playback and return to the
  start screen. Without it, every reset relaunches the app.
- `shot <name>` (optional): capture the composited screen. Some content, such as native child
  windows from another process, is blank in a `PrintWindow` capture. Reply with an absolute path,
  or with a bare name inside `app.shot_dir`.
- Push `ui-settled` once the interface stops changing after an action; otherwise kritic waits 5 s.

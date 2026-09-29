"""ux_critic adapter for desktop (Windows UI Automation) and Unity (the app's own event socket) apps.

A persistent process driven by kritic over JSON lines: one request per line on stdin, one reply per
line on stdout, progress events on stderr. Same contract as harness/ux_web.mjs:

  {op:"start", kind, exe, args, cwd, env, window_title_re, app_events, ready_event, ready_timeout_s,
   stall_s, ignore, allow_destructive, out_dir}
  {op:"reset"}                        relaunch (Unity: the app's `home` command when it has one)
  {op:"observe", shot?, marks?}       -> {obs: {fingerprint, title, text, viewport, controls, screenshot?, marked?, events}}
  {op:"act", id}
  {op:"close"}                        -> {hygiene}: processes and ports the run left alive (then killed)

Unity apps draw their own UI, so UI Automation sees nothing: the app must answer these harness
commands over its event socket (`id|verb|argument`, replied to with a `command` event carrying
`id`, `outcome` and `detail`):
  controls <label>  writes {"screen":[w,h],"controls":[{path,label,text,tooltip,role,rect,enabled,selected}],
                    "texts":[...]} to a JSON file (rect: x,y,w,h in window pixels, origin top-left) and
                    replies with the file's path;
  click <path>      presses the control at that hierarchy path;
  home              (optional) closes dialogs and menus and returns to the start screen.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import sys
from pathlib import Path

import desktop_test as dt
import hygiene
import unity_mode as um


def reply(obj: dict) -> None:
    sys.stdout.write(json.dumps(obj, default=str) + "\n")
    sys.stdout.flush()


def fingerprint(parts: list[str]) -> str:
    # digits folded: a clock or counter ticking is not a new screen
    return hashlib.sha1(re.sub(r"\d+", "0", "\n".join(parts)).encode("utf-8", "replace")).hexdigest()[:16]


def assign_ids(controls: list[dict]) -> None:
    """role:label#n, stable across replays (unless the adapter already has a unique path)."""
    seen: dict[str, int] = {}
    for c in controls:
        if c.get("id"):
            continue
        key = f"{c['role']}:{c['label'] or c['text']}"
        seen[key] = seen.get(key, 0) + 1
        c["id"] = f"{key}#{seen[key]}"


def mark_skips(inp: dict, controls: list[dict]) -> None:
    ignore = [re.compile(p) for p in inp.get("ignore") or []]
    for c in controls:
        if any(r.search(c["label"]) for r in ignore):
            c["skip"] = "ignored"
        elif c["label"].strip().lower() in dt.TITLEBAR:
            c["skip"] = "window chrome"
        elif not inp.get("allow_destructive") and dt.DESTRUCTIVE.search(c["label"]):
            c["skip"] = "destructive"


def draw_marks(src: str, dst: str, controls: list[dict]) -> str:
    """Set-of-marks: number the actionable controls in the order kritic lists them to the novice."""
    from PIL import Image, ImageDraw

    img = Image.open(src).convert("RGB")
    d = ImageDraw.Draw(img)
    for n, c in enumerate((c for c in controls if c["enabled"] and not c.get("skip")), 1):
        x, y, w, h = c["rect"]
        d.rectangle([x, y, x + w, y + h], outline=(230, 0, 0), width=2)
        d.rectangle([x, max(0, y - 14), x + 8 + 7 * len(str(n)), max(14, y)], fill=(230, 0, 0))
        d.text((x + 3, max(0, y - 14)), str(n), fill=(255, 255, 255))
    Path(dst).parent.mkdir(parents=True, exist_ok=True)
    img.save(dst)
    return dst


class DesktopApp:
    """UI Automation: every visible actionable control of every app window, relaunched on reset."""

    def __init__(self, inp: dict):
        self.inp = inp
        self.out_dir = Path(inp["out_dir"])
        self.app = None
        self.win = None
        self.by_id: dict = {}

    def reset(self) -> None:
        self.close()
        report = {"crashes": []}
        self.app, self.win = dt.start_app(self.inp, self.out_dir, report)
        if self.win is None:
            raise RuntimeError(report.get("error") or "no window")

    def _scan(self) -> tuple[list[dict], list[str]]:
        origin = self.win.rectangle()
        controls, texts, self.by_id = [], [], {}
        wrappers = []
        for w in self.app.windows():
            texts.append(w.window_text() or "")
            for d in w.descendants():
                try:
                    ei = d.element_info
                    if not d.is_visible():
                        continue
                    if ei.control_type == "Text":
                        texts.append(ei.name or "")
                        continue
                    if ei.control_type not in dt.ACTIONABLE:
                        continue
                    parent = d.parent()
                    if parent is not None and parent.element_info.control_type in dt.COMPOSITE_PARENTS:
                        continue
                    r = d.rectangle()
                    label = (ei.name or "").strip()[:80]
                    controls.append({
                        "label": label, "text": label, "tooltip": _help_text(d), "role": ei.control_type,
                        "rect": [r.left - origin.left, r.top - origin.top, r.width(), r.height()],
                        "enabled": bool(d.is_enabled()), "inline": False, "selected": _selected(d),
                    })
                    wrappers.append(d)
                except Exception:  # noqa: BLE001, S112  (control went away mid-scan)
                    continue
        assign_ids(controls)
        mark_skips(self.inp, controls)
        self.by_id = {c["id"]: w for c, w in zip(controls, wrappers)}
        return controls, texts

    def observe(self, req: dict) -> dict:
        controls, texts = self._scan()
        rect = self.win.rectangle()
        obs = {
            "fingerprint": fingerprint(texts[:1] + [f"{c['id']}|{c['enabled']}|{c['selected']}" for c in controls] + texts[1:]),
            "title": self.win.window_text(), "url": "", "text": " ".join(t for t in texts if t)[:20000],
            "viewport": [rect.width(), rect.height()], "controls": controls, "events": [],
        }
        if req.get("shot") or req.get("marks"):
            path = req.get("shot") or str(Path(req["marks"]).with_suffix(".plain.png"))
            Path(path).parent.mkdir(parents=True, exist_ok=True)
            if dt.shot(self.win, Path(path)):
                if req.get("shot"):
                    obs["screenshot"] = path
                if req.get("marks"):
                    obs["marked"] = draw_marks(path, req["marks"], controls)
                    if not req.get("shot"):
                        Path(path).unlink(missing_ok=True)
        return obs

    def act(self, cid: str) -> None:
        self._scan()
        c = self.by_id.get(cid)
        if c is None:
            raise RuntimeError(f"no control {cid} on this screen")
        dt.act(c, bool(self.inp.get("use_mouse")))
        dt.wait_idle(self.win.handle, 2.0)

    def close(self) -> None:
        if self.app is not None:
            self.app.close()
            self.app = self.win = None


def _help_text(d) -> str:
    try:
        return (d.element_info.element.CurrentHelpText or "").strip()[:160]
    except Exception:  # noqa: BLE001
        return ""


def _selected(d) -> bool:
    try:
        ct = d.element_info.control_type
        if ct == "CheckBox":
            return d.get_toggle_state() == 1
        if ct in ("TabItem", "ListItem", "RadioButton"):
            return bool(d.is_selected())
    except Exception:  # noqa: BLE001, S110  (no selection pattern: not selected)
        pass
    return False


class UnityApp:
    """Drives a Unity app through its own harness commands over its event socket."""

    def __init__(self, inp: dict):
        self.inp = inp
        self.out_dir = Path(inp["out_dir"])
        args, self.log_path = um.unity_args(inp.get("args", []), self.out_dir, 1600, 900)
        self.launch_inp = {**inp, "args": args}
        self.stall = float(inp.get("stall_s", 15))
        self.app = self.win = self.events = None
        self.since = 0
        self.n = 0
        self.has_home = True
        self.has_shot = True

    def _cmd(self, verb: str, arg: str = "", timeout_s: float | None = None) -> str:
        self.n += 1
        cid = f"ux{self.n}"
        since = self.events.mark()
        self.events.send(f"{cid}|{verb}|{arg}")
        found = self.events.find(rf'"id"\s*:\s*"{cid}"', timeout_s or self.stall, since=since)
        if found is None:
            raise TimeoutError(f"app stalled: no reply to {verb} within {timeout_s or self.stall:.0f}s")
        ev = json.loads(found[0])
        if ev.get("outcome") == "err":
            raise RuntimeError(f"{verb}: {ev.get('detail', '')[:300]}")
        return str(ev.get("detail", ""))

    def _harness_dir(self) -> str:
        """Where the app's `shot` files land: `app.shot_dir` in ux.toml (an absolute reply needs none)."""
        return self.inp.get("shot_dir") or ""

    def reset(self) -> None:
        if self.app is not None and self.app.alive() and self.has_home:
            try:
                self._cmd("home")
                self.since = self.events.mark()
                return
            except RuntimeError as e:
                if "unknown verb" not in str(e):
                    raise
                self.has_home = False
        self.close()
        busy = dt.port_holder(self.inp["app_events"])
        if busy:
            raise RuntimeError(f"app_events port already in use: {busy}")
        report = {"crashes": []}
        self.app, self.win = dt.start_app(self.launch_inp, self.out_dir, report)
        if self.win is None:
            raise RuntimeError(report.get("error") or "no window")
        self.events = um.AppEvents(self.inp["app_events"], dt.progress, float(self.inp.get("ready_timeout_s", 120)))
        ready = dt.wait_until_ready(self.inp, self.app, self.log_path, self.events)
        if not ready.get("matched") and self.inp.get("ready_event"):
            raise RuntimeError(f"app not ready: {ready.get('failed') or ready.get('stalled') or 'timed out'}")
        self.since = self.events.mark()

    def observe(self, req: dict) -> dict:
        path = Path(self._cmd("controls", f"ux-{self.n}"))
        data = json.loads(path.read_text(encoding="utf-8"))
        path.unlink(missing_ok=True)
        controls = [{
            "id": c["path"], "label": (c.get("label") or "").strip()[:80], "text": (c.get("text") or "").strip()[:80],
            "tooltip": (c.get("tooltip") or "").strip()[:160], "role": c.get("role") or "button", "rect": c["rect"],
            "enabled": bool(c.get("enabled", True)), "inline": False, "selected": bool(c.get("selected")),
        } for c in data.get("controls", [])]
        mark_skips(self.inp, controls)
        # The app's harness `click` presses buttons only; sliders, fields and scrollbars are audited, not pressed.
        for c in controls:
            if c["role"] != "Button" and not c.get("skip"):
                c["skip"] = "not clickable"
        # Entries are strings, or {text, rect} objects (an app may send where each text sits).
        texts = [t.get("text", "") if isinstance(t, dict) else t for t in data.get("texts", [])]
        texts = [t for t in texts if t]
        events = self.events.since(self.since)
        obs = {
            "fingerprint": fingerprint([f"{c['id']}|{c['label']}|{c['enabled']}|{c['selected']}" for c in controls] + texts),
            "title": self.win.window_text(), "url": "", "text": " ".join(texts)[:20000],
            "viewport": data.get("screen") or [0, 0], "controls": controls, "events": events,
        }
        if req.get("shot") or req.get("marks"):
            shot = req.get("shot") or str(Path(req["marks"]).with_suffix(".plain.png"))
            Path(shot).parent.mkdir(parents=True, exist_ok=True)
            if self._shot(Path(shot)):
                if req.get("shot"):
                    obs["screenshot"] = shot
                if req.get("marks"):
                    obs["marked"] = draw_marks(shot, req["marks"], controls)
                    if not req.get("shot"):
                        Path(shot).unlink(missing_ok=True)
        return obs

    def _shot(self, target: Path) -> bool:
        """The app's own `shot` when it has one: PrintWindow draws only the app's window, so a
        native child window from another process (an embedded preview) came out blank
        and read as a broken preview. The app's capture reads the composited screen."""
        if self.has_shot:
            try:
                name = self._cmd("shot", f"ux-shot-{self.n}", timeout_s=max(self.stall, 20))
                produced = Path(self._harness_dir()) / name
                if produced.is_file():
                    shutil.move(str(produced), target)
                    produced.with_suffix("").with_suffix(".hierarchy.txt").unlink(missing_ok=True)
                    return True
            except RuntimeError as e:
                if "unknown verb" not in str(e):
                    raise
                self.has_shot = False
        return dt.shot(self.win, target)

    def act(self, cid: str) -> None:
        since = self.events.mark()
        self._cmd("click", cid)
        # An app may push ui-settled once the interface stops changing; an app without it just waits 5 s
        self.events.find(r'"event"\s*:\s*"ui-settled"', 5, since=since)

    def close(self) -> None:
        if self.events is not None:
            self.events.close()
            self.events = None
        if self.app is not None:
            self.app.close()
            self.app = self.win = None


def main() -> None:
    target = None
    before = roots = None
    for raw in sys.stdin.buffer:
        line = raw.decode("utf-8", "replace").strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except ValueError as e:
            reply({"ok": False, "error": f"bad request JSON: {e}"})
            continue
        op = req.get("op")
        if op == "close":
            break
        try:
            if op == "start":
                Path(req["out_dir"]).mkdir(parents=True, exist_ok=True)
                roots = hygiene.roots_for(req)
                before = hygiene.snapshot(roots) if roots else None
                target = UnityApp(req) if req.get("kind") == "unity" else DesktopApp(req)
                reply({"ok": True})
            elif target is None:
                raise RuntimeError("start first")
            elif op == "reset":
                target.reset()
                reply({"ok": True})
            elif op == "observe":
                if target.win is None:
                    raise RuntimeError("reset first")
                reply({"ok": True, "obs": target.observe(req)})
            elif op == "act":
                target.act(str(req["id"]))
                reply({"ok": True})
            else:
                raise RuntimeError(f"unknown op {op}")
        except Exception as e:  # noqa: BLE001  (reported to kritic; the loop keeps serving)
            reply({"ok": False, "error": f"{type(e).__name__}: {str(e)[:300]}"})
    if target is not None:
        target.close()
    leaks = hygiene.leaks(before, roots) if before is not None else None
    reply({"ok": True, "hygiene": leaks})


if __name__ == "__main__":
    main()

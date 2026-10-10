"""ux_critic adapter: the desktop path live against the WinForms fixture, the Unity command protocol
against a fake event stream. uv run --extra test pytest -q
"""
import json
import subprocess  # noqa: S404; intended local tool invocation  # nosec B404
import sys
import threading
from pathlib import Path

import pytest

import ux_adapter as ux

HERE = Path(__file__).parent
ADAPTER = HERE.parent / "ux_adapter.py"
FIXTURE = HERE / "fixture_app.ps1"


class Session:
    def __init__(self):
        self.p = subprocess.Popen([sys.executable, str(ADAPTER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,  # noqa: S603; explicit argv, fixed test harness # nosec B603
                                  stderr=subprocess.DEVNULL, text=True, encoding="utf-8")

    def call(self, req: dict) -> dict:
        self.p.stdin.write(json.dumps(req) + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())


@pytest.mark.skipif(sys.platform != "win32", reason="Windows UI Automation only")
def test_desktop_observe_act_reset_close(tmp_path):
    s = Session()
    try:
        start = {"op": "start", "kind": "desktop", "exe": "powershell.exe",
                 "args": ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(FIXTURE), "-Mode", "good",
                          "-StateFile", str(tmp_path / "state.json")],
                 "window_title_re": "Kritic Fixture", "out_dir": str(tmp_path), "ignore": ["^Settings"]}
        assert s.call(start)["ok"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        assert s.call({"op": "reset"})["ok"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        o = s.call({"op": "observe", "shot": str(tmp_path / "home.png"), "marks": str(tmp_path / "m.png")})["obs"]
        by = {c["label"]: c for c in o["controls"]}
        assert by["Save"]["enabled"] and by["Save"]["rect"][2] >= 24, by["Save"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        assert by["Settings..."]["skip"] == "ignored"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        assert Path(o["screenshot"]).exists() and Path(o["marked"]).exists()  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        assert "Status: idle" in o["text"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        assert s.call({"op": "act", "id": by["Save"]["id"]})["ok"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        after = s.call({"op": "observe"})["obs"]
        assert "Status: saved" in after["text"] and after["fingerprint"] != o["fingerprint"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        assert s.call({"op": "reset"})["ok"]  # relaunch: back to the start state  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        assert "Status: idle" in s.call({"op": "observe"})["obs"]["text"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        missing = s.call({"op": "act", "id": "Button:Nope#1"})
        assert not missing["ok"] and "no control" in missing["error"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        s.p.stdin.write(json.dumps({"op": "close"}) + "\n")
        s.p.stdin.flush()
        final = json.loads(s.p.stdout.readline())
        assert final["ok"] and final["hygiene"]["processes"] == [], final  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        assert s.p.wait(timeout=30) == 0  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    finally:
        if s.p.poll() is None:
            s.p.kill()


def test_ids_skips_and_fingerprint():
    cs = [{"label": "Save", "text": "Save", "role": "Button"}, {"label": "Save", "text": "Save", "role": "Button"},
          {"label": "", "text": "", "role": "Button"}, {"label": "Delete all data", "text": "", "role": "Button"},
          {"label": "Close", "text": "", "role": "Button"}, {"label": "Beta", "text": "", "role": "Button"}]
    ux.assign_ids(cs)
    assert [c["id"] for c in cs[:3]] == ["Button:Save#1", "Button:Save#2", "Button:  # 1"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    ux.mark_skips({"ignore": ["^Beta$"]}, cs)
    assert [c.get("skip") for c in cs[3:]] == ["destructive", "window chrome", "ignored"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert ux.fingerprint(["clock 10:41"]) == ux.fingerprint(["clock 10:42"])  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert ux.fingerprint(["Editor"]) != ux.fingerprint(["Library"])  # noqa: S101; test assertion, not runtime authorization  # nosec B101


class FakeEvents:
    """Answers harness command lines like a harnessed Unity app does: a `command` event with the id."""

    def __init__(self, answers):
        self.answers, self.events, self.count = answers, [], 0
        self._cond = threading.Condition()

    def mark(self):
        return self.count

    def send(self, line):
        cid, verb, arg = line.split("|", 2)
        outcome, detail = self.answers(verb, arg)
        self.events.append(json.dumps({"event": "command", "id": cid, "outcome": outcome, "detail": detail}))
        self.count += 1

    def since(self, index):
        return self.events[index:]

    def find(self, pattern, timeout_s, since=0):
        import re
        for i, e in enumerate(self.events[since:], since):
            if re.search(pattern, e):
                return e, i
        return None


def test_unity_protocol_controls_click_and_home(tmp_path):
    dump = tmp_path / "c.json"
    clicks = []

    def answers(verb, arg):
        if verb == "controls":
            dump.write_text(json.dumps({"screen": [1600, 900], "texts": ["Library"], "controls": [
                {"path": "Root/Bar/Composer", "label": "Composer", "tooltip": "Open the Composer", "role": "tab",
                 "rect": [10, 5, 90, 28], "enabled": True},
                {"path": "Root/Bar/Icon", "label": "", "tooltip": "", "rect": [110, 5, 20, 20], "enabled": True}]}))
            return "ok", str(dump)
        if verb == "click":
            clicks.append(arg)
            return "ok", "clicked " + arg
        return "err", f"unknown verb '{verb}'"

    u = ux.UnityApp({"out_dir": str(tmp_path), "app_events": "ws://127.0.0.1:1", "stall_s": 1})
    u.events = FakeEvents(answers)

    class Win:
        def window_text(self):
            return "Studio"

    class App:
        def alive(self):
            return True

    u.win, u.app = Win(), App()
    o = u.observe({})
    assert [c["id"] for c in o["controls"]] == ["Root/Bar/Composer", "Root/Bar/Icon"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert o["viewport"] == [1600, 900] and o["text"] == "Library" and not dump.exists()  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    u.act("Root/Bar/Composer")
    assert clicks == ["Root/Bar/Composer"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    # no `home` verb: the adapter notes it and relaunches instead (not exercised here)
    with pytest.raises(RuntimeError, match="unknown verb"):
        u._cmd("home")


def test_volatile_text_does_not_make_a_new_screen():
    v = ux.compile_volatile(["^Validation", "^Loading"])
    assert ux.is_volatile("Validation passed", v) and ux.is_volatile("Loading: waveform", v) and not ux.is_volatile("Composer", v)  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def _unity(tmp_path, answers, **extra):
    u = ux.UnityApp({"out_dir": str(tmp_path), "app_events": "ws://127.0.0.1:1", "stall_s": 1, **extra})
    u.events = FakeEvents(answers)

    class Win:
        def window_text(self):
            return "Studio"

    class App:
        def alive(self):
            return True

    u.win, u.app = Win(), App()
    return u


def test_unity_observe_waits_for_the_app_to_stop_being_busy(tmp_path):
    dump, reads = tmp_path / "c.json", []

    def answers(verb, arg):
        reads.append(verb)
        busy = "opening the map: waveform" if len(reads) < 3 else ""
        dump.write_text(json.dumps({"screen": [1600, 900], "texts": ["Loading: waveform" if busy else "Ready"], "busy": busy, "controls": []}))
        return "ok", str(dump)

    u = _unity(tmp_path, answers, volatile=["^Loading", "^Ready$"])
    o = u.observe({})
    assert len(reads) == 3 and o["busy"] == "", reads  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    # a wait that runs out is reported on the observation, not hidden
    reads.clear()
    u2 = _unity(tmp_path, lambda verb, arg: (dump.write_text(json.dumps({"screen": [1, 1], "texts": [], "busy": "starting the preview", "controls": []})), ("ok", str(dump)))[1], busy_timeout_s=0.3)
    assert u2.observe({})["busy"] == "starting the preview"  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_pixel_diff_reports_the_changed_fraction_and_honours_masked_regions(tmp_path):
    PIL = pytest.importorskip("PIL.Image")
    from PIL import Image, ImageDraw

    def shot(name, box=None):
        img = Image.new("RGB", (100, 100), "black")
        if box:
            ImageDraw.Draw(img).rectangle(box, fill="white")
        path = tmp_path / name
        img.save(path)
        return str(path)

    base, same, changed = shot("a.png"), shot("b.png"), shot("c.png", (0, 0, 49, 99))
    assert ux.pixel_diff(base, same)["fraction"] == 0.0  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    d = ux.pixel_diff(base, changed)
    assert 0.49 < d["fraction"] < 0.51 and d["bbox"][0] == 0 and d["bbox"][2] == 50, d  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    # the animated half of the window is masked out (fractions of the window)
    assert ux.pixel_diff(base, changed, [[0.0, 0.0, 0.5, 1.0]])["fraction"] == 0.0  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    other = tmp_path / "small.png"
    Image.new("RGB", (50, 50)).save(other)
    assert ux.pixel_diff(base, str(other))["sizes_differ"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101

"""ux_critic adapter: the desktop path live against the WinForms fixture, the Unity command protocol
against a fake event stream. uv run --extra test pytest -q
"""
import json
import subprocess
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
        self.p = subprocess.Popen([sys.executable, str(ADAPTER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
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
        assert s.call(start)["ok"]
        assert s.call({"op": "reset"})["ok"]
        o = s.call({"op": "observe", "shot": str(tmp_path / "home.png"), "marks": str(tmp_path / "m.png")})["obs"]
        by = {c["label"]: c for c in o["controls"]}
        assert by["Save"]["enabled"] and by["Save"]["rect"][2] >= 24, by["Save"]
        assert by["Settings..."]["skip"] == "ignored"
        assert Path(o["screenshot"]).exists() and Path(o["marked"]).exists()
        assert "Status: idle" in o["text"]
        assert s.call({"op": "act", "id": by["Save"]["id"]})["ok"]
        after = s.call({"op": "observe"})["obs"]
        assert "Status: saved" in after["text"] and after["fingerprint"] != o["fingerprint"]
        assert s.call({"op": "reset"})["ok"]  # relaunch: back to the start state
        assert "Status: idle" in s.call({"op": "observe"})["obs"]["text"]
        missing = s.call({"op": "act", "id": "Button:Nope#1"})
        assert not missing["ok"] and "no control" in missing["error"]
        s.p.stdin.write(json.dumps({"op": "close"}) + "\n")
        s.p.stdin.flush()
        final = json.loads(s.p.stdout.readline())
        assert final["ok"] and final["hygiene"]["processes"] == [], final
        assert s.p.wait(timeout=30) == 0
    finally:
        if s.p.poll() is None:
            s.p.kill()


def test_ids_skips_and_fingerprint():
    cs = [{"label": "Save", "text": "Save", "role": "Button"}, {"label": "Save", "text": "Save", "role": "Button"},
          {"label": "", "text": "", "role": "Button"}, {"label": "Delete all data", "text": "", "role": "Button"},
          {"label": "Close", "text": "", "role": "Button"}, {"label": "Beta", "text": "", "role": "Button"}]
    ux.assign_ids(cs)
    assert [c["id"] for c in cs[:3]] == ["Button:Save#1", "Button:Save#2", "Button:#1"]
    ux.mark_skips({"ignore": ["^Beta$"]}, cs)
    assert [c.get("skip") for c in cs[3:]] == ["destructive", "window chrome", "ignored"]
    assert ux.fingerprint(["clock 10:41"]) == ux.fingerprint(["clock 10:42"])
    assert ux.fingerprint(["Editor"]) != ux.fingerprint(["Library"])


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
    assert [c["id"] for c in o["controls"]] == ["Root/Bar/Composer", "Root/Bar/Icon"]
    assert o["viewport"] == [1600, 900] and o["text"] == "Library" and not dump.exists()
    u.act("Root/Bar/Composer")
    assert clicks == ["Root/Bar/Composer"]
    # no `home` verb: the adapter notes it and relaunches instead (not exercised here)
    with pytest.raises(RuntimeError, match="unknown verb"):
        u._cmd("home")

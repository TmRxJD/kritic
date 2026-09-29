"""Live tests: drive the WinForms fixture through desktop_test.py. Windows only.
uv run --extra test pytest -q
"""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

HERE = Path(__file__).parent
HARNESS = HERE.parent / "desktop_test.py"
FIXTURE = HERE / "fixture_app.ps1"
pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="Windows UI Automation only")


def run(inp: dict) -> dict:
    out = subprocess.run([sys.executable, str(HARNESS)], input=json.dumps(inp), capture_output=True, text=True, timeout=240)
    assert out.stdout, out.stderr
    return json.loads(out.stdout)


def app(mode: str, state: str, **extra) -> dict:
    return {
        "exe": "powershell.exe",
        "args": ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(FIXTURE), "-Mode", mode, "-StateFile", state],
        "out_dir": tempfile.mkdtemp(prefix="kritic-desk-"),
        "window_title_re": "Kritic Fixture",
        **extra,
    }


def by_name(r: dict, name: str) -> dict:
    return next(i for i in r["interactions"] if i["element"]["name"] == name)


def test_good_app_clean_and_persistent(tmp_path):
    state = str(tmp_path / "state.json")
    r = run(app("good", state, persist=[{"auto_id": "nameBox"}, {"auto_id": "darkBox"}]))
    assert r["ok"], r.get("error")
    assert r["controls_found"] >= 2  # persisted controls (nameBox, darkBox) are excluded from exploration
    assert r["crashes"] == [] and r["error_dialogs"] == [] and r["hangs"] == []
    assert by_name(r, "Save")["result"] == "ok"
    assert all(p["persisted"] for p in r["persistence"]), r["persistence"]
    assert any(Path(s).exists() for s in r["screenshots"])


def test_broken_app_reports_every_defect(tmp_path):
    r = run(app("broken", str(tmp_path / "state.json"), persist=[{"auto_id": "nameBox"}]))
    assert r["ok"], r.get("error")
    assert by_name(r, "Load report")["result"] == "error_dialog"
    assert "Object reference" in r["error_dialogs"][0]["text"]
    assert r["persistence"][0]["persisted"] is False
    assert by_name(r, "Does nothing")["result"] == "no_effect"
    assert by_name(r, "Delete all data")["result"] == "skipped"
    assert by_name(r, "Zz crash")["result"] == "crash"
    assert r["crashes"][0]["exit_code"] not in (0, None)  # 3, or 0xC000041D when Exit runs inside a UIA callback
    assert "widget cache failed" in r["stderr_tail"]


def test_hang_detected(tmp_path):
    r = run(app("hang", str(tmp_path / "state.json"), hang_timeout_s=3))
    assert r["ok"], r.get("error")
    assert by_name(r, "Freeze")["result"] == "hang"
    assert r["hangs"]


def test_startup_failure_is_reported():
    r = run({"exe": "powershell.exe", "args": ["-NoProfile", "-Command", "exit 7"], "out_dir": tempfile.mkdtemp(), "startup_timeout_s": 10})
    assert r["ok"] is False
    assert "code 7" in r["error"]


def test_resource_sampler_measures_a_busy_process():
    busy = [sys.executable, "-c", "import time\nx=[]\nt=time.time()\nwhile time.time()-t<30:\n    x.append(bytearray(200000)); sum(range(20000))"]
    r = run({"mode": "resource", "command": busy, "duration_s": 5, "interval_ms": 250})
    assert r["ok"], r.get("error")
    s = r["stats"]
    assert s["cpu_avg_pct"] > 20
    assert s["rss_slope_mb_per_min"] > 10, s
    assert s["samples"] >= 10


def test_resource_requires_consent_to_attach():
    r = run({"mode": "resource", "pid": os.getpid(), "duration_s": 1})
    assert r["ok"] is False and "allow_attach" in r["error"]


def test_flow_through_modal_dialog(tmp_path):
    r = run(app("good", str(tmp_path / "state.json"), explore=False, flows=[
        {"name": "set nickname", "steps": [
            {"do": "click", "target": {"name": "Settings..."}},
            {"do": "expect_window", "value": "^Settings$"},
            {"do": "fill", "target": {"auto_id": "nickBox", "window": "^Settings$"}, "value": "ada"},
            {"do": "click", "target": {"name": "OK", "window": "^Settings$"}},
            {"do": "expect_text", "target": {"auto_id": "status"}, "value": "nick=ada"},
        ]},
        {"name": "missing control", "steps": [
            {"do": "click", "target": {"name": "No Such Button"}, "timeout_ms": 1000},
            {"do": "click", "target": {"name": "Save"}},
        ]},
    ]))
    assert r["ok"], r.get("error")
    ok, missing = r["flows"]
    assert ok["passed"], ok["steps"]
    assert [s["result"] for s in missing["steps"]] == ["fail", "skipped"]
    assert "not found" in missing["steps"][0]["error"]


def test_unity_soak_detects_crash_and_parses_player_log(tmp_path):
    r = run(app("unitysim", str(tmp_path / "s.json"), unity=True, soak_s=8, shot_every_s=1,
                unity_steps=[{"do": "key", "value": "space"}, {"do": "click", "target": {"x": 0.5, "y": 0.5}}]))
    assert r["ok"], r.get("error")
    u = r["unity"]
    assert r["crashes"] and r["crashes"][0]["exit_code"] == 5
    assert u["log"]["counts"]["exception"] == 1
    assert [s["result"] for s in u["steps"]] == ["ok", "ok"]
    assert r["screenshots"], "periodic screenshots"
    assert "frames" in u  # PresentMon result or the reason it could not run

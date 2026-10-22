"""Live tests: drive the WinForms fixture through desktop_test.py. Windows only.
uv run --extra test pytest -q
"""
import json
import os
import subprocess  # noqa: S404; intended local tool invocation  # nosec B404
import sys
import tempfile
from pathlib import Path

import pytest

HERE = Path(__file__).parent
HARNESS = HERE.parent / "desktop_test.py"
FIXTURE = HERE / "fixture_app.ps1"
pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="Windows UI Automation only")


def run(inp: dict) -> dict:
    out = subprocess.run([sys.executable, str(HARNESS)], input=json.dumps(inp), capture_output=True, text=True, timeout=240)  # noqa: S603; explicit argv, fixed test harness # nosec B603
    assert out.stdout, out.stderr  # noqa: S101; test assertion, not runtime authorization  # nosec B101
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
    assert r["ok"], r.get("error")  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert r["controls_found"] >= 2  # persisted controls (nameBox, darkBox) are excluded from exploration  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert r["crashes"] == [] and r["error_dialogs"] == [] and r["hangs"] == []  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert by_name(r, "Save")["result"] == "ok"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert all(p["persisted"] for p in r["persistence"]), r["persistence"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert any(Path(s).exists() for s in r["screenshots"])  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_broken_app_reports_every_defect(tmp_path):
    r = run(app("broken", str(tmp_path / "state.json"), persist=[{"auto_id": "nameBox"}]))
    assert r["ok"], r.get("error")  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert by_name(r, "Load report")["result"] == "error_dialog"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "Object reference" in r["error_dialogs"][0]["text"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert r["persistence"][0]["persisted"] is False  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert by_name(r, "Does nothing")["result"] == "no_effect"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert by_name(r, "Delete all data")["result"] == "skipped"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert by_name(r, "Zz crash")["result"] == "crash"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert r["crashes"][0]["exit_code"] not in (0, None)  # 3, or 0xC000041D when Exit runs inside a UIA callback  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "widget cache failed" in r["stderr_tail"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_hang_detected(tmp_path):
    r = run(app("hang", str(tmp_path / "state.json"), hang_timeout_s=3))
    assert r["ok"], r.get("error")  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert by_name(r, "Freeze")["result"] == "hang"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert r["hangs"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_startup_failure_is_reported():
    r = run({"exe": "powershell.exe", "args": ["-NoProfile", "-Command", "exit 7"], "out_dir": tempfile.mkdtemp(), "startup_timeout_s": 10})
    assert r["ok"] is False  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "code 7" in r["error"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_resource_sampler_measures_a_busy_process():
    busy = [sys.executable, "-c", "import time\nx=[]\nt=time.time()\nwhile time.time()-t<30:\n    x.append(bytearray(200000)); sum(range(20000))"]
    r = run({"mode": "resource", "command": busy, "duration_s": 5, "interval_ms": 250})
    assert r["ok"], r.get("error")  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    s = r["stats"]
    assert s["cpu_avg_pct"] > 20  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert s["rss_slope_mb_per_min"] > 10, s  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert s["samples"] >= 10  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_resource_requires_consent_to_attach():
    r = run({"mode": "resource", "pid": os.getpid(), "duration_s": 1})
    assert r["ok"] is False and "allow_attach" in r["error"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101


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
    assert r["ok"], r.get("error")  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    ok, missing = r["flows"]
    assert ok["passed"], ok["steps"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert [s["result"] for s in missing["steps"]] == ["fail", "skipped"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "not found" in missing["steps"][0]["error"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_unity_soak_detects_crash_and_parses_player_log(tmp_path):
    r = run(app("unitysim", str(tmp_path / "s.json"), unity=True, soak_s=8, shot_every_s=1,
                unity_steps=[{"do": "key", "value": "space"}, {"do": "click", "target": {"x": 0.5, "y": 0.5}}]))
    assert r["ok"], r.get("error")  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    u = r["unity"]
    assert r["crashes"] and r["crashes"][0]["exit_code"] == 5  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert u["log"]["counts"]["exception"] == 1  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert [s["result"] for s in u["steps"]] == ["ok", "ok"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert r["screenshots"], "periodic screenshots"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "frames" in u  # PresentMon result or the reason it could not run  # noqa: S101; test assertion, not runtime authorization  # nosec B101

"""Offline tests for Unity support (no Unity player needed)."""
from pathlib import Path

import unity_mode as um

PLAYER_LOG = """Mono path[0] = 'C:/Game/Game_Data/Managed'
Initialize engine version: 6000.6.2f1
Direct3D: detected blah
NullReferenceException: Object reference not set to an instance of an object
  at Enemy.Update () [0x00000] in <abc>:0
NullReferenceException: Object reference not set to an instance of an object
  at Enemy.Update () [0x00000] in <abc>:0
Assertion failed: health must be positive
Error: could not load save file slot 2
The referenced script (Unknown) on this Behaviour is missing!
Fallback handler could not load library C:/Game/Mono/data-0x1.dll
Crash!!!
"""


def test_parse_player_log_counts_and_dedupes():
    r = um.parse_player_log(PLAYER_LOG)
    c = r["counts"]
    assert c["exception"] == 2  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert r["samples"]["exception"] == ["NullReferenceException: Object reference not set to an instance of an object"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert c["assertion"] == 1 and c["error"] == 1 and c["native_crash"] == 1 and c["missing"] == 1  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "Fallback handler" not in str(r["samples"])  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_clean_log_has_no_counts():
    assert um.parse_player_log("Initialize engine version\nUnloading 3 unused Assets\n")["counts"] == {}  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_unity_args_add_windowed_and_log(tmp_path):
    args, log = um.unity_args(["-myflag"], tmp_path, 1280, 720)
    assert args[:1] == ["-myflag"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert "-logFile" in args and log == tmp_path / "player.log"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert args[args.index("-screen-fullscreen") + 1] == "0"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    args2, log2 = um.unity_args(["-logFile", "C:/x.log", "-screen-width", "800"], tmp_path, 1280, 720)
    assert log2 == Path("C:/x.log") and args2.count("-logFile") == 1 and "-screen-height" not in args2  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_is_unity_exe(tmp_path):
    (tmp_path / "Game.exe").write_bytes(b"")
    assert not um.is_unity_exe(str(tmp_path / "Game.exe"))  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    (tmp_path / "UnityPlayer.dll").write_bytes(b"")
    assert um.is_unity_exe(str(tmp_path / "Game.exe"))  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_frame_stats_from_presentmon_csv(tmp_path):
    p = tmp_path / "f.csv"
    rows = ["Application,ProcessID,TimeInSeconds,FrameTime"] + [f"Game.exe,1,{i/60:.3f},16.6" for i in range(98)]
    rows += ["Game.exe,1,2.0,50.0", "Game.exe,1,2.1,120.0"]
    p.write_text("\n".join(rows) + "\n", encoding="utf-8")
    s = um.frame_stats(p)
    assert s["ok"] and s["frames"] == 100 and s["column"] == "FrameTime"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert s["stutters"] == 2 and s["max_ms"] == 120.0  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert 50 < s["avg_fps"] < 60  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert um.frame_stats(tmp_path / "missing.csv")["ok"] is False  # noqa: S101; test assertion, not runtime authorization  # nosec B101


# --------------------------------------------------------------------------- app event stream

def _serve(handler):
    """A local WebSocket app on a free port, run in a background thread."""
    import threading

    from websockets.sync.server import serve

    server = serve(handler, "127.0.0.1", 0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"ws://127.0.0.1:{server.socket.getsockname()[1]}"


def _harnessed_app(ws):
    """Behaves like a harnessed Unity app: announces readiness, answers `id|verb|arg` lines."""
    import json

    ws.send(json.dumps({"event": "status", "detail": "ready"}))
    for line in ws:
        cid, verb, arg = (line.split("|", 2) + ["", ""])[:3]
        outcome = "err" if arg == "Missing" else "ok"
        ws.send(json.dumps({"event": "command", "id": cid, "outcome": outcome, "detail": f"{verb} {arg}"}))
        ws.send(json.dumps({"event": "ui-settled", "detail": "ready"}))


def test_app_events_ready_send_and_await():
    import json

    import pytest

    server, url = _serve(_harnessed_app)
    relayed = []
    ev = um.AppEvents(url, lambda e, **d: relayed.append(e), connect_timeout_s=5)
    try:
        assert ev.wait_for(r'"detail": "ready"', 5) is not None  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        ok = um.run_event_step(ev, {"do": "send", "value": "1|click|Menu Analysis"})
        assert '"id": "1"' in ok["event"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        mark = ev.mark()
        reply = um.run_event_step(ev, {"do": "send", "value": "2|click|Item Lyrics…"})
        assert json.loads(reply["event"])["detail"] == "click Item Lyrics…"  # non-ASCII survives  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        got = um.run_event_step(ev, {"do": "await_event", "value": "ui-settled", "since": mark})
        assert "ui-settled" in got["event"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        with pytest.raises(RuntimeError, match="Missing"):
            um.run_event_step(ev, {"do": "send", "value": "3|assert|Missing"})
        assert "app_event" in relayed and "app_events_connected" in relayed  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    finally:
        ev.close()
        server.shutdown()


def test_await_event_ignores_events_before_since():
    server, url = _serve(_harnessed_app)
    ev = um.AppEvents(url, lambda e, **d: None, connect_timeout_s=5)
    try:
        assert ev.wait_for("ready", 5)  # noqa: S101; test assertion, not runtime authorization  # nosec B101
        import pytest

        with pytest.raises(TimeoutError):
            um.run_event_step(ev, {"do": "await_event", "value": "ready", "since": ev.mark(), "timeout_ms": 300})
    finally:
        ev.close()
        server.shutdown()


def test_unreachable_app_events_reports_error():
    ev = um.AppEvents("ws://127.0.0.1:9", lambda e, **d: None, connect_timeout_s=1)
    ev._thread.join(5)
    s = ev.summary()
    assert s["connected"] is False and s["error"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    ev.close()


def test_command_id():
    assert um.command_id("7|click|X") == "7"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert um.command_id("no pipes") is None  # noqa: S101; test assertion, not runtime authorization  # nosec B101



def test_frame_stats_minimal_mode_ms_time_columns(tmp_path):
    # PresentMon without display/GPU tracking: MsBetweenPresents plus CPUStartTimeInMs (milliseconds).
    rows = ["Application,ProcessID,SwapChainAddress,TimeInMs,MsBetweenPresents,CPUStartTimeInMs"]
    rows += [f"app.exe,1,0xA,{t},10.0,{t}" for t in range(0, 3000, 10)]
    csv_path = tmp_path / "pm.csv"
    csv_path.write_text("\n".join(rows))
    s = um.frame_stats(csv_path, since_s=1.0)
    assert s["ok"] and s["column"] == "MsBetweenPresents"  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert s["warmup_frames_excluded"] == 100 and s["frames"] == 200  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    assert s["avg_fps"] == 100.0  # noqa: S101; test assertion, not runtime authorization  # nosec B101


def test_await_after_send_ignores_events_before_the_reply():
    events = um.AppEvents.__new__(um.AppEvents)
    import threading
    events._cond = threading.Condition(); events.events = []; events.count = 0; events.keep = 500; events._stop = False
    events.connected = True; events.error = None
    sent = []
    events._ws = type("W", (), {"send": lambda self, line: sent.append(line)})()

    def push(text):
        with events._cond:
            events.events.append(text); events.count += 1; events._cond.notify_all()

    push('{"event":"ui-settled","t":1}')          # stale: before the command
    threading.Timer(0.05, lambda: (push('{"event":"command","id":"1","outcome":"ok"}'), push('{"event":"ui-settled","t":2}'))).start()
    result = um.run_event_step(events, {"do": "send", "value": "1|click|X", "timeout_ms": 2000})
    assert result["after"] == 2 and sent == ["1|click|X"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101
    got = um.run_event_step(events, {"do": "await_event", "value": '"ui-settled"', "since": result["after"], "timeout_ms": 2000})
    assert '"t":2' in got["event"]  # noqa: S101; test assertion, not runtime authorization  # nosec B101

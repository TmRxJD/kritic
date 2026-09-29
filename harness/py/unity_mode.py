"""Unity player support for desktop_test.

Unity renders its own UI, so Windows UI Automation sees no controls to exercise. Instead:
  * launch windowed with a known log file (-screen-fullscreen 0 -logFile <out>/player.log),
  * soak for `soak_s`: watch for exit, UnityCrashHandler, hangs; screenshot periodically;
    optionally capture per-frame timing with PresentMon (needs admin / Performance Log Users),
  * optional input steps posted to the Unity window (keys; clicks at window-relative points),
  * parse Player.log for exceptions, errors, assertion failures and native crash markers.
In-game UI flows by object name need AltTester in the build (not included).
"""
from __future__ import annotations

import csv
import re
import shutil
import subprocess
import time
from pathlib import Path

LOG_PATTERNS = [
    ("exception", re.compile(r"^(\w+\.)*\w*Exception\b.*|^\s*UnityException\b.*", re.M)),
    ("error", re.compile(r"^(?:Error|ERROR)\b.*|.*\bLogError\b.*", re.M)),
    ("assertion", re.compile(r"^Assertion failed.*", re.M)),
    ("native_crash", re.compile(r"Crash!!!|Native Crash Reporting|Received signal SIGSEGV|UnityCrashHandler", re.M)),
    ("shader", re.compile(r"^Shader error.*|shader.*failed to compile", re.M | re.I)),
    ("missing", re.compile(r"The referenced script .* is missing|MissingReferenceException|Missing Prefab", re.M)),
]
NOISE = re.compile(r"Fallback handler could not load library|d3d11: failed to create|Direct3D: detected|<RI>|FMOD", re.I)


def is_unity_exe(exe: str) -> bool:
    p = Path(exe)
    return p.suffix.lower() == ".exe" and (p.parent / "UnityPlayer.dll").exists()


def unity_args(user_args: list[str], out_dir: Path, width: int, height: int) -> tuple[list[str], Path]:
    """Windowed at a fixed size with the log in out_dir, unless the caller set these already."""
    args = list(user_args)
    low = [a.lower() for a in args]
    log = out_dir / "player.log"
    if "-logfile" in low:
        log = Path(args[low.index("-logfile") + 1])
    else:
        args += ["-logFile", str(log)]
    if "-screen-fullscreen" not in low:
        args += ["-screen-fullscreen", "0"]
    if "-screen-width" not in low:
        args += ["-screen-width", str(width), "-screen-height", str(height)]
    return args, log


def parse_player_log(text: str, limit: int = 20) -> dict:
    """Count and sample problem lines. Each distinct message counted once in `samples`."""
    counts: dict[str, int] = {}
    samples: dict[str, list[str]] = {}
    for kind, rx in LOG_PATTERNS:
        hits = [m.group(0).strip() for m in rx.finditer(text) if not NOISE.search(m.group(0))]
        if hits:
            counts[kind] = len(hits)
            uniq = list(dict.fromkeys(h[:300] for h in hits))
            samples[kind] = uniq[:limit]
    return {"counts": counts, "samples": samples, "lines": text.count("\n")}


def presentmon_exe(bin_dir: str | None) -> str | None:
    for cand in ([str(Path(bin_dir) / "PresentMon.exe")] if bin_dir else []) + [shutil.which("PresentMon") or ""]:
        if cand and Path(cand).exists():
            return cand
    return None


def start_presentmon(exe: str, process_name: str, seconds: int, csv_path: Path) -> subprocess.Popen:
    return subprocess.Popen(
        # Present-to-present timing only. Tracking frames to the display, GPU work or input needs
        # events a non-elevated session does not reliably get; when they are missing PresentMon
        # emits no rows at all (seen 2026-09-27: 0 frames from a 700 fps app). The metrics used
        # here (MsBetweenPresents / MsBetweenAppStart) do not need them.
        [exe, "--process_name", process_name, "--output_file", str(csv_path), "--timed", str(seconds),
         "--terminate_after_timed", "--no_console_stats", "--no_track_display", "--no_track_gpu",
         "--no_track_input", "--stop_existing_session", "--session_name", "kritic_presentmon"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
    )


FRAME_COLUMNS = ("FrameTime", "MsBetweenPresents", "MsBetweenAppStart", "MsBetweenDisplayChange")


# (column, seconds per unit): PresentMon names its time columns differently per metrics mode.
TIME_COLUMNS = (("CPUStartTime", 1.0), ("TimeInSeconds", 1.0), ("CPUStartTimeInMs", 0.001), ("TimeInMs", 0.001))
STUTTER_DEF = "frame time > max(2 x median, budget_ms) within the measured window on the dominant swapchain"


def _num(v) -> float | None:
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def frame_stats(csv_path: Path, since_s: float = 0.0, budget_ms: float | None = None) -> dict:
    """Per-frame timing summary from a PresentMon CSV.

    Window: rows whose start time (seconds since capture start) is >= ``since_s``;
    frames before it (splash, loading) are counted in ``warmup_frames_excluded``.
    Only the swapchain with the most frames in the window is measured, so an
    overlay or second window does not mix its cadence in.
    """
    if not csv_path.exists():
        return {"ok": False, "error": "PresentMon wrote no frames (see presentmon.output)"}
    with csv_path.open(newline="", encoding="utf-8", errors="replace") as f:
        rows = list(csv.DictReader(f))
    if not rows:
        return {"ok": False, "error": "PresentMon captured no frames (process not presenting?)"}
    col = next((c for c in FRAME_COLUMNS if c in rows[0]), None)
    if col is None:
        return {"ok": False, "error": f"no frame-time column in {list(rows[0])[:12]}"}
    tcol, tscale = next(((c, k) for c, k in TIME_COLUMNS if c in rows[0]), (None, 1.0))
    warm = 0
    if tcol and since_s > 0:
        kept = [r for r in rows if (_num(r.get(tcol)) or 0.0) * tscale >= since_s]
        warm = len(rows) - len(kept)
        rows = kept
    chains: dict[str, int] = {}
    for r in rows:
        chains[r.get("SwapChainAddress") or "?"] = chains.get(r.get("SwapChainAddress") or "?", 0) + 1
    main = max(chains, key=chains.get) if chains else None
    ms = sorted(x for r in rows if (r.get("SwapChainAddress") or "?") == main and (x := _num(r.get(col))) is not None)
    if not ms:
        return {"ok": False, "error": f"no frames after the {since_s:.1f}s warmup window ({warm} excluded)",
                "warmup_frames_excluded": warm}
    n = len(ms)
    pct = lambda p: ms[min(n - 1, int(p * (n - 1)))]  # noqa: E731
    median = pct(0.5)
    avg = sum(ms) / n
    limit = max(2 * median, budget_ms or 0.0)
    stutters = sum(1 for x in ms if x > limit)
    return {
        "ok": True, "column": col, "frames": n,
        "window_start_s": round(since_s, 3), "warmup_frames_excluded": warm,
        "swapchains": len(chains), "other_swapchain_frames": sum(chains.values()) - n,
        "avg_fps": round(1000 / avg, 1), "median_ms": round(median, 2),
        "p95_ms": round(pct(0.95), 2), "p99_ms": round(pct(0.99), 2), "max_ms": round(ms[-1], 2),
        "low_1pct_fps": round(1000 / pct(0.99), 1),
        "stutter_limit_ms": round(limit, 2), "stutter_def": STUTTER_DEF,
        "stutters": stutters, "stutter_pct": round(100 * stutters / n, 2),
    }


def crash_dialog_open(pids: set[int], top_windows, text) -> str | None:
    """Unity players start UnityCrashHandler64.exe at launch as a watchdog, so its presence means
    nothing. A crash shows up as the player exiting, or the handler opening a visible window."""
    import psutil

    handlers = set()
    for pid in pids:
        try:
            if "unitycrashhandler" in psutil.Process(pid).name().lower():
                handlers.add(pid)
        except psutil.Error:
            continue
    wins = top_windows(handlers) if handlers else []
    return text(wins[0]) or "crash handler window" if wins else None


WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP = 0x0100, 0x0101, 0x0201, 0x0202
VK = {"space": 0x20, "enter": 0x0D, "escape": 0x1B, "tab": 0x09, "left": 0x25, "up": 0x26, "right": 0x27, "down": 0x28}


def post_input(user32, hwnd: int, step: dict) -> None:
    """Unity-player input without moving the real mouse: key presses and clicks posted to the window.
    Clicks use window-relative fractions {x: 0..1, y: 0..1}. Unity reads posted keys reliably; posted
    clicks work for the legacy Input Manager and uGUI with the old input module, not always the new
    Input System. For object-level UI flows, use AltTester in the build."""
    do = step.get("do")
    if do == "key":
        k = str(step.get("value", "")).lower()
        vk = VK.get(k, ord(k.upper()[0]) if k else 0)
        user32.PostMessageW(hwnd, WM_KEYDOWN, vk, 0)
        time.sleep(0.05)
        user32.PostMessageW(hwnd, WM_KEYUP, vk, 0xC0000001)
    elif do == "click":
        import ctypes
        from ctypes import wintypes

        r = wintypes.RECT()
        user32.GetClientRect(hwnd, ctypes.byref(r))
        x = int(float(step["target"]["x"]) * (r.right - r.left))
        y = int(float(step["target"]["y"]) * (r.bottom - r.top))
        lp = (y << 16) | (x & 0xFFFF)
        user32.PostMessageW(hwnd, WM_LBUTTONDOWN, 1, lp)
        time.sleep(0.05)
        user32.PostMessageW(hwnd, WM_LBUTTONUP, 0, lp)
    elif do == "wait":
        time.sleep(float(step.get("value", 500)) / 1000)
    else:
        raise ValueError(f"unknown unity step '{do}' (key | click | wait)")


# --------------------------------------------------------------------------- app event stream
#
# An app that publishes its own state over a WebSocket (e.g. ws://127.0.0.1:4733)
# tells the tester when it is ready and when each action has finished, instead of the tester
# re-reading Player.log or sleeping. Every app event is also relayed to kritic's own stream.


class AppEvents:
    """Background WebSocket client: records the app's events, relays them as progress, and lets
    steps wait for a matching event or send a command line."""

    def __init__(self, url: str, progress, connect_timeout_s: float = 60, keep: int = 500):
        self.url = url
        self.progress = progress
        self.connect_timeout_s = connect_timeout_s
        self.keep = keep
        self.events: list[str] = []
        self.count = 0
        self.error: str | None = None
        self.connected = False
        self._ws = None
        self._cond = __import__("threading").Condition()
        self._stop = False
        self._thread = __import__("threading").Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        from websockets.sync.client import connect

        deadline = time.time() + self.connect_timeout_s
        while not self._stop and time.time() < deadline:
            try:
                # Never let one attempt outlast the whole connect budget.
                # No keepalive pings: a Unity app busy for a few frames (generation, loading) misses them and
                # the connection drops. A silent app is caught by the callers' stall limits instead.
                with connect(self.url, open_timeout=max(0.2, min(2.0, deadline - time.time())), max_size=2**22, ping_interval=None) as ws:
                    self._ws = ws
                    with self._cond:
                        self.connected = True
                        self._cond.notify_all()
                    self.progress("app_events_connected", url=self.url)
                    for message in ws:
                        text = message if isinstance(message, str) else message.decode("utf-8", "replace")
                        with self._cond:
                            self.events.append(text)
                            self.count += 1
                            if len(self.events) > self.keep * 2:
                                del self.events[: len(self.events) - self.keep]
                            self._cond.notify_all()
                        self.progress("app_event", data=text[:500])
                    return
            except Exception as e:  # noqa: BLE001  (app not listening yet: retry until the deadline)
                self.error = f"{type(e).__name__}: {e}"[:200]
                time.sleep(0.25)
        if not self.connected and not self.error:
            self.error = f"no connection within {self.connect_timeout_s:.0f}s"
        with self._cond:
            self._cond.notify_all()

    def mark(self) -> int:
        """Index to wait from, taken *before* acting, so a fast reply cannot be missed."""
        with self._cond:
            return self.count

    def wait_for(self, pattern: str, timeout_s: float, since: int = 0) -> str | None:
        found = self.find(pattern, timeout_s, since)
        return found[0] if found else None

    def find(self, pattern: str, timeout_s: float, since: int = 0) -> tuple[str, int] | None:
        """The first event at or after absolute index `since` matching `pattern`, with its index."""
        rx = re.compile(pattern)
        deadline = time.time() + timeout_s
        with self._cond:
            while True:
                base = self.count - len(self.events)
                start = max(0, since - base)
                for offset, text in enumerate(self.events[start:]):
                    if rx.search(text):
                        return text, base + start + offset
                since = self.count
                left = deadline - time.time()
                if left <= 0 or self._stop:
                    return None
                self._cond.wait(min(left, 0.5))

    def since(self, index: int, limit: int = 200) -> list[str]:
        """Events at or after absolute index `index` (the last `limit` of them still kept)."""
        with self._cond:
            base = self.count - len(self.events)
            return self.events[max(0, index - base):][-limit:]

    def send(self, line: str) -> None:
        if not self.connected or self._ws is None:
            raise RuntimeError(f"not connected to {self.url}: {self.error or 'no connection'}")
        self._ws.send(line)

    def summary(self) -> dict:
        with self._cond:
            return {"url": self.url, "connected": self.connected, "events": self.count,
                    "last": self.events[-20:],
                    "error": None if self.connected else (self.error or "not connected yet")}

    def close(self) -> None:
        self._stop = True
        if self._ws is not None:
            with __import__("contextlib").suppress(Exception):  # already closed by the app
                self._ws.close()


def command_id(line: str) -> str | None:
    """Harness command lines are `id|verb|argument`; the result event carries that id."""
    head = line.split("|", 1)
    return head[0] if len(head) == 2 and head[0] else None


def run_event_step(events: AppEvents | None, step: dict) -> dict:
    """`send`: send a command line and wait for its result event (`until` regex, default the
    command's id); an `"outcome":"err"` result fails the step. `await_event`: wait for `value`."""
    do = step.get("do")
    timeout_s = float(step.get("timeout_ms", 30000)) / 1000
    if events is None:
        raise ValueError(f"step '{do}' needs app_events (the app's WebSocket URL)")
    t0 = time.time()
    if do == "await_event":
        got = events.wait_for(str(step["value"]), timeout_s, since=int(step.get("since", 0)))
        if got is None:
            raise TimeoutError(f"no event matching {step['value']!r} within {timeout_s:.0f}s")
        return {"event": got[:300], "ms": round((time.time() - t0) * 1000)}
    if do == "send":
        line = str(step["value"])
        cid = command_id(line)
        # Tolerant of JSON spacing: some apps send compact JSON, others do not.
        until = step.get("until") or (rf'"id"\s*:\s*"{re.escape(cid)}"' if cid else None)
        since = events.mark()
        events.send(line)
        if not until:
            return {"ms": 0}
        found = events.find(until, timeout_s, since=since)
        if found is None:
            raise TimeoutError(f"no reply to {line!r} within {timeout_s:.0f}s")
        got, index = found
        if re.search(r'"outcome"\s*:\s*"err"', got):
            raise RuntimeError(f"{line!r} -> {got[:300]}")
        # A following await_event looks only after this reply: whatever the command
        # caused comes after it, and anything earlier is not its effect.
        return {"event": got[:300], "ms": round((time.time() - t0) * 1000), "after": index + 1}
    raise ValueError(f"unknown event step '{do}'")

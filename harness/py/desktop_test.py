"""Deterministic desktop tester + resource sampler for kritic. No LLM involvement.

Input: one JSON object on stdin. Output: one JSON object on stdout.

  mode "desktop_test": launch a Windows desktop app, walk its UI Automation tree, exercise every
      visible enabled control through UIA patterns (Invoke/Toggle/Value/SelectionItem/ExpandCollapse,
      never the real mouse unless use_mouse), and record crashes, hangs, error dialogs, other dialogs,
      no-effect controls, persistence failures (across a restart), stderr, and screenshots.
  mode "resource": launch a command (or attach to a pid) and sample the process tree: CPU, memory,
      handles, threads, I/O, plus whole-GPU utilization/memory via nvidia-smi.
"""
from __future__ import annotations

import ctypes
import json
import re
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

import psutil

DESTRUCTIVE = re.compile(
    r"\b(delete|remove|destroy|erase|wipe|format|uninstall|log ?out|sign ?out|exit|quit|shut ?down|"
    r"restart|reboot|pay|purchase|buy|reset all)\b",
    re.I,
)
TITLEBAR = {"close", "minimize", "maximize", "restore", "system", "system menu"}
ERROR_WORDS = re.compile(r"\b(error|exception|fatal|unhandled|crash|failed|failure|stopped working)\b", re.I)
ACTIONABLE = {
    "Button", "SplitButton", "Hyperlink", "CheckBox", "RadioButton", "Edit", "ComboBox",
    "TabItem", "ListItem", "MenuItem", "Slider", "Spinner",
}


def emit(obj: dict) -> None:
    sys.stdout.write(json.dumps(obj, default=str))
    sys.stdout.flush()


def progress(event: str, **data) -> None:
    """One JSON line on stderr; the MCP server streams these to its WebSocket as they happen."""
    sys.stderr.write(json.dumps({"event": event, **data}, default=str) + "\n")
    sys.stderr.flush()


def tree_pids(pid: int) -> set[int]:
    try:
        p = psutil.Process(pid)
        return {pid} | {c.pid for c in p.children(recursive=True)}
    except psutil.Error:
        return {pid}


def is_hung(hwnd: int) -> bool:
    try:
        return bool(ctypes.windll.user32.IsHungAppWindow(hwnd))
    except Exception:
        return False


# --------------------------------------------------------------------------- win32 watchdog

user32 = ctypes.windll.user32 if sys.platform == "win32" else None
WM_CLOSE = 0x0010
EnumProc = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p) if sys.platform == "win32" else None


def _text(hwnd) -> str:
    n = user32.GetWindowTextLengthW(hwnd)
    buf = ctypes.create_unicode_buffer(n + 1)
    user32.GetWindowTextW(hwnd, buf, n + 1)
    return buf.value


def _pid(hwnd) -> int:
    pid = ctypes.c_ulong()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    return pid.value


def top_windows(pids: set[int]) -> list[int]:
    found = []

    def cb(hwnd, _):
        if user32.IsWindowVisible(hwnd) and _pid(hwnd) in pids:
            found.append(hwnd)
        return True

    user32.EnumWindows(EnumProc(cb), 0)
    return found


def child_texts(hwnd) -> list[str]:
    texts = []

    def cb(h, _):
        t = _text(h)
        if t and t not in ("OK", "&OK", "Cancel", "&Yes", "&No", "Yes", "No"):
            texts.append(t)
        return len(texts) < 8

    user32.EnumChildWindows(hwnd, EnumProc(cb), 0)
    return texts


def responds(hwnd: int, timeout_s: float) -> bool:
    """True if the window's thread handles WM_NULL within timeout_s (SMTO_ABORTIFHUNG)."""
    result = ctypes.c_size_t()
    ok = user32.SendMessageTimeoutW(hwnd, 0x0000, 0, 0, 0x0002, int(timeout_s * 1000), ctypes.byref(result))
    return bool(ok) or not user32.IsWindow(hwnd)


def wait_idle(hwnd: int, max_s: float, min_s: float = 0.12) -> float:
    """Wait until the UI thread answers promptly twice in a row (or max_s). Returns seconds waited."""
    t0 = time.time()
    time.sleep(min_s)
    calm = 0
    while time.time() - t0 < max_s:
        calm = calm + 1 if responds(hwnd, 0.25) else 0
        if calm >= 2:
            break
        time.sleep(0.05)
    return time.time() - t0


class Watchdog(threading.Thread):
    """Win32-only (no COM): closes new top-level windows of the app, measures UI-thread hangs,
    and kills the app if it stays hung past kill_s so the blocked action returns."""

    def __init__(self, app, known: set, main_hwnd: int, hang_s: float, kill_s: float):
        super().__init__(daemon=True)
        self.app, self.known, self.main = app, known, main_hwnd
        self.hang_s, self.kill_s = hang_s, kill_s
        self.dialogs: list[dict] = []
        self.hung_s = 0.0
        self.killed = False
        self._halt = threading.Event()

    def stop(self):
        self._halt.set()
        self.join(5)

    def run(self):
        hung_since = None
        while not self._halt.is_set():
            if not self.app.alive():
                return
            pids = tree_pids(self.app.pid)
            for h in top_windows(pids):
                if h in self.known or h == self.main:
                    continue
                self.known.add(h)
                self.dialogs.append({"title": _text(h)[:120], "text": " | ".join(child_texts(h))[:400]})
                user32.PostMessageW(h, WM_CLOSE, 0, 0)
            if user32.IsHungAppWindow(self.main):
                hung_since = hung_since or time.time()
                held = time.time() - hung_since
                if held >= self.hang_s:
                    self.hung_s = held
                if held >= self.kill_s:
                    self.killed = True
                    for pid in pids:
                        try:
                            psutil.Process(pid).kill()
                        except psutil.Error:
                            pass
                    return
            elif hung_since is not None:
                hung_since = None
            time.sleep(0.25)


# --------------------------------------------------------------------------- desktop test


def default_cwd(exe: str) -> str | None:
    """Apps expect to start in their own folder (relative data/helper paths); PATH commands keep ours."""
    p = Path(exe)
    return str(p.parent) if p.is_absolute() and p.parent.exists() else None


class App:
    def __init__(self, inp: dict, out_dir: Path):
        self.inp = inp
        self.out_dir = out_dir
        self.proc: subprocess.Popen | None = None
        self.pid: int | None = None
        self.stderr_path = out_dir / "stderr.txt"

    def launch(self) -> None:
        from pywinauto import Desktop  # noqa: F401  (import early so failures surface here)

        cmd = [self.inp["exe"], *self.inp.get("args", [])]
        self.errf = open(self.stderr_path, "w", encoding="utf-8", errors="replace")
        import os

        env = {**os.environ, **{str(k): str(v) for k, v in (self.inp.get("env") or {}).items()}}
        self.proc = subprocess.Popen(
            cmd, cwd=self.inp.get("cwd") or default_cwd(self.inp["exe"]), stdout=self.errf, stderr=self.errf,
            stdin=subprocess.DEVNULL, env=env
        )
        self.pid = self.proc.pid

    def windows(self):
        """Visible top-level windows of the app's process tree. Enumerated with Win32 (instant);
        each is wrapped for UIA only when found."""
        from pywinauto import Desktop

        out = []
        for h in top_windows(tree_pids(self.pid)):
            try:
                out.append(Desktop(backend="uia").window(handle=h).wrapper_object())
            except Exception:  # noqa: BLE001  (window closed meanwhile)
                continue
        return out

    def main_window(self, timeout: float):
        deadline = time.time() + timeout
        title_re = self.inp.get("window_title_re")
        from pywinauto import Desktop

        while time.time() < deadline:
            if self.proc and self.proc.poll() is not None:
                return None
            for h in top_windows(tree_pids(self.pid)):
                t = _text(h)
                if t and (not title_re or re.search(title_re, t)):
                    try:
                        return Desktop(backend="uia").window(handle=h).wrapper_object()
                    except Exception:  # noqa: BLE001
                        continue
            time.sleep(0.1)
        return None

    def alive(self) -> bool:
        return self.proc is not None and self.proc.poll() is None

    def close(self) -> None:
        if not self.proc:
            return
        # Snapshot the tree first: once the app exits its children are no longer reachable from it.
        family = [p for p in tree_pids(self.pid) if p != self.pid]
        for w in self.windows():
            try:
                w.close()
            except Exception:
                pass
        # Two different things: the app honouring the close (its windows go), and the process
        # object ending. A process can outlive its windows while the kernel finishes a driver
        # request no user-mode code can cancel (seen: a HID device timing out for 5 s), which is
        # not the app ignoring the close. close_timed_out means the windows stayed; the exit
        # timings are reported separately.
        started = time.time()
        closed_at = None
        while time.time() - started < 5:
            if self.proc.poll() is not None or not top_windows([self.pid]):
                closed_at = time.time()
                break
            time.sleep(0.05)
        self.close_ms = round((closed_at - started) * 1000) if closed_at else None
        self.close_timed_out = closed_at is None
        try:
            self.proc.wait(timeout=10 if closed_at else 0.1)
            self.exit_ms = round((time.time() - started) * 1000)
        except subprocess.TimeoutExpired:
            self.exit_ms = None
            self.exit_timed_out = True
            victims = []
            for pid in tree_pids(self.pid):
                try:
                    victims.append(psutil.Process(pid))
                    victims[-1].kill()
                except psutil.Error:
                    pass
            psutil.wait_procs(victims, timeout=5)
        # Children still alive after the app is gone leak ports, files and CPU. Helpers that watch
        # their parent (e.g. UnityCrashHandler) exit by themselves shortly after: allow them 3 s.
        self.orphans = []
        alive = []
        for pid in family:
            try:
                alive.append(psutil.Process(pid))
            except psutil.Error:
                pass
        _, lingering = psutil.wait_procs(alive, timeout=3)
        for proc in lingering:
            try:
                self.orphans.append(f"{proc.pid} {proc.name()}")
                proc.kill()
            except psutil.Error:
                pass
        try:
            self.errf.close()
        except Exception:
            pass

    def stderr_tail(self) -> str:
        try:
            return self.stderr_path.read_text(encoding="utf-8", errors="replace")[-4000:]
        except OSError:
            return ""


def describe(c) -> dict:
    ei = c.element_info
    return {
        "control_type": ei.control_type or "",
        "name": (ei.name or "").strip()[:80],
        "auto_id": ei.automation_id or "",
    }


def _win32_signature(hwnd: int) -> tuple[str, int]:
    parts = []

    def cb(h, _):
        parts.append(f"{_text(h)}|{user32.IsWindowVisible(h)}|{user32.IsWindowEnabled(h)}")
        return True

    user32.EnumChildWindows(hwnd, EnumProc(cb), 0)
    return str(hash("\n".join(parts))), len(parts)


def signature(win) -> str:
    """Hash of the window's controls (text, visibility, enabled). Win32 child windows when the app
    has them (WinForms, Win32, most toolkits): instant. UIA tree walk only as a fallback (WPF)."""
    sig, n = _win32_signature(win.handle)
    if n >= 2:
        return sig
    parts = []
    try:
        for d in win.descendants():
            try:
                ei = d.element_info
                parts.append(f"{ei.control_type}|{ei.name}|{d.is_enabled()}")
            except Exception:  # noqa: BLE001
                continue
    except Exception:  # noqa: BLE001
        pass
    return str(hash("\n".join(parts)))


COMPOSITE_PARENTS = {"ComboBox", "TitleBar", "ScrollBar", "Spinner", "MenuBar"}


def candidates(win) -> list:
    out = []
    for d in win.descendants():
        try:
            ei = d.element_info
            if ei.control_type not in ACTIONABLE or not d.is_visible() or not d.is_enabled():
                continue
            parent = d.parent()
            if parent is not None and parent.element_info.control_type in COMPOSITE_PARENTS:
                continue  # e.g. a ComboBox's own "Open" button, title-bar Minimize/Close
            out.append(d)
        except Exception:
            continue
    return out


def find_again(win, desc: dict):
    """Re-resolve a control after the UI may have changed."""
    for d in candidates(win):
        if describe(d) == desc:
            return d
    return None


def act(c, use_mouse: bool) -> str:
    ct = c.element_info.control_type
    if ct == "CheckBox":
        c.toggle()
        return "toggle"
    if ct == "RadioButton":
        c.select()
        return "select"
    if ct == "Edit":
        c.set_edit_text("kritic test")
        return "fill"
    if ct == "ComboBox":
        try:
            items = c.texts()
            c.select(1 if len(items) > 1 else 0)
        except Exception:
            c.expand()
            time.sleep(0.2)
            c.collapse()
        return "select"
    if ct in ("TabItem", "ListItem"):
        c.select()
        return "select"
    if ct in ("Slider", "Spinner"):
        return "none"
    try:
        click_async(c)
        return "invoke"
    except Exception:
        if use_mouse:
            c.click_input()
            return "click"
        raise


def print_window(hwnd: int, path: Path) -> bool:
    """Capture a window's own pixels with PrintWindow(PW_RENDERFULLCONTENT): correct even when the
    window is partly covered, and works for DirectX/Unity windows (Windows 8.1+)."""
    from PIL import Image
    from ctypes import wintypes

    gdi32 = ctypes.windll.gdi32
    r = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    w, h = r.right - r.left, r.bottom - r.top
    if w <= 0 or h <= 0:
        return False
    hdc = user32.GetWindowDC(hwnd)
    mem = gdi32.CreateCompatibleDC(hdc)
    bmp = gdi32.CreateCompatibleBitmap(hdc, w, h)
    old = gdi32.SelectObject(mem, bmp)
    try:
        if not user32.PrintWindow(hwnd, mem, 2):  # PW_RENDERFULLCONTENT
            return False

        class BITMAPINFOHEADER(ctypes.Structure):
            _fields_ = [("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int32), ("biHeight", ctypes.c_int32),
                        ("biPlanes", ctypes.c_uint16), ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32),
                        ("biSizeImage", ctypes.c_uint32), ("biXPelsPerMeter", ctypes.c_int32), ("biYPelsPerMeter", ctypes.c_int32),
                        ("biClrUsed", ctypes.c_uint32), ("biClrImportant", ctypes.c_uint32)]

        bi = BITMAPINFOHEADER(ctypes.sizeof(BITMAPINFOHEADER), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
        buf = ctypes.create_string_buffer(w * h * 4)
        if not gdi32.GetDIBits(mem, bmp, 0, h, buf, ctypes.byref(bi), 0):
            return False
        Image.frombuffer("RGBA", (w, h), buf, "raw", "BGRA", 0, 1).convert("RGB").save(path)
        return True
    finally:
        gdi32.SelectObject(mem, old)
        gdi32.DeleteObject(bmp)
        gdi32.DeleteDC(mem)
        user32.ReleaseDC(hwnd, hdc)


def shot(win, path: Path) -> str | None:
    try:
        if print_window(win.handle, path):
            return str(path)
    except Exception:  # noqa: BLE001
        pass
    try:
        win.capture_as_image().save(path)  # fallback: screen region
        return str(path)
    except Exception:  # noqa: BLE001
        return None


def skip_reason(inp: dict, app: App, desc: dict) -> str | None:
    name = desc["name"]
    if name.strip().lower() in TITLEBAR:
        return "window chrome"
    if not inp.get("allow_destructive") and DESTRUCTIVE.search(f"{name} {desc['auto_id']}"):
        return "looks destructive (set allow_destructive)"
    if not app.alive():
        return "app no longer running"
    return None


def run_action(inp: dict, app: App, win, c, rec: dict, known: set) -> tuple[Watchdog, Exception | None]:
    """Perform one action with the watchdog running; returns the stopped watchdog and any error.

    The action runs on this thread (UIA/COM objects live here). WinForms executes Invoke
    synchronously, so an action that opens a modal dialog blocks until it closes: the Win32-only
    watchdog records and closes new windows, and flags/kills hangs.
    """
    hang_s = inp.get("hang_timeout_s", 5)
    dog = Watchdog(app, known, win.handle, hang_s, inp.get("hang_kill_s", 20))
    dog.start()
    err = None
    t_act = time.time()
    try:
        rec["action"] = act(c, bool(inp.get("use_mouse")))
    except Exception as e:  # noqa: BLE001
        err = e
    rec["duration_ms"] = int((time.time() - t_act) * 1000)
    # UIA actions execute on the app's UI thread: an action that took longer than hang_s froze
    # the UI that long (unless a watchdog-closed dialog was what held it).
    if rec["duration_ms"] / 1000 >= hang_s and not dog.dialogs:
        dog.hung_s = max(dog.hung_s, rec["duration_ms"] / 1000)
    wait_idle(win.handle, inp.get("wait_ms", 1500) / 1000)
    if err is not None or not user32.IsWindow(win.handle):
        for _ in range(10):  # an error or a vanished window may be the process dying
            if not app.alive():
                break
            time.sleep(0.2)
    if app.alive() and not responds(win.handle, hang_s):
        dog.hung_s = max(dog.hung_s, float(hang_s))
    dog.stop()
    return dog, err


def classify(inp: dict, app: App, dog: Watchdog, err, rec: dict, report: dict) -> bool:
    """Fill rec["result"] from what happened. Returns True when exploration must stop."""
    name = rec["element"]["name"]
    for d in dog.dialogs:
        entry = {"title": d["title"], "text": d["text"], "after": name}
        if ERROR_WORDS.search(d["title"]) or ERROR_WORDS.search(d["text"]):
            report["error_dialogs"].append(entry)
            rec["result"] = "error_dialog"
            rec["errors"].append(f"{d['title']}: {d['text']}"[:300])
        else:
            report["dialogs"].append(entry)
    if not app.alive():
        code = app.proc.poll()
        if dog.killed:
            rec.update(result="hang", errors=[f"not responding for {inp.get('hang_kill_s', 20)}s; killed"])
            report["hangs"].append({"after": name, "exit_code": code})
        else:
            rec.update(result="crash", errors=[f"process exited with code {code}"])
            report["crashes"].append({"after": name, "exit_code": code})
        return True
    if dog.hung_s > 0:
        rec.update(result="hang", errors=[f"UI thread not responding for >= {dog.hung_s:.0f}s after action"])
        report["hangs"].append({"after": name, "seconds": round(dog.hung_s, 1)})
        return True  # further UIA calls would block on the hung thread
    if err is not None and rec["result"] == "ok":
        rec.update(result="skipped", detail=f"no usable UIA pattern: {str(err).splitlines()[0][:120]}")
    return False


def start_app(inp: dict, out_dir: Path, report: dict):
    """Launch and wait for the main window. Returns (app, win) or (app, None) with report filled."""
    app = App(inp, out_dir)
    try:
        app.launch()
    except Exception as e:  # noqa: BLE001
        report.update(ok=False, error=f"failed to launch: {e}")
        return app, None
    report["pid"] = app.pid
    win = app.main_window(inp.get("startup_timeout_s", 30))
    if win is None:
        code = app.proc.poll()
        app.close()
        err = f"no window appeared within {inp.get('startup_timeout_s', 30)}s"
        if code is not None:
            report["crashes"].append({"after": "startup", "exit_code": code})
            err = f"app exited during startup with code {code}"
        report.update(ok=False, error=err, stderr_tail=app.stderr_tail(), exit_code=code)
    return app, win


# --------------------------------------------------------------------------- multi-step flows
#
# flow: {name, steps: [step], continue_on_error?}   (each flow runs in a fresh launch)
# step: {do, target?: {name?, auto_id?, control_type?, window?}, value?, timeout_ms?}
#   do: click | fill | select | check | uncheck | toggle | press (value=keys, pywinauto syntax)
#       | wait_for | expect_text (value substring of the control's value/name) | expect_window
#       (value=title regex) | close_window (value=title regex) | wait (value=ms) | screenshot

BM_CLICK = 0x00F5


def find_target(app: "App", target: dict, timeout_s: float):
    """Find a control in any of the app's windows (dialogs included) with UIA's own search."""
    from pywinauto import Desktop

    crit = {k2: target[k1] for k1, k2 in (("auto_id", "auto_id"), ("name", "title"), ("control_type", "control_type")) if target.get(k1)}
    if not (target.get("auto_id") or target.get("name")):
        return None
    win_re = target.get("window")
    deadline = time.time() + timeout_s
    while True:
        for h in top_windows(tree_pids(app.pid)):
            if win_re and not re.search(win_re, _text(h) or ""):
                continue
            try:
                spec = Desktop(backend="uia").window(handle=h).child_window(**crit)
                if spec.exists(timeout=0):
                    return spec.wrapper_object()
            except Exception:  # noqa: BLE001  (window went away)
                continue
        if time.time() > deadline:
            return None
        time.sleep(0.1)


def control_text(c) -> str:
    for getter in ("get_value", "window_text"):
        try:
            v = getattr(c, getter)()
            if v:
                return str(v)
        except Exception:  # noqa: BLE001
            continue
    return c.element_info.name or ""


def click_async(c) -> None:
    """Click without stalling.

    A UIA Invoke that opens a modal dialog doesn't return until the dialog closes (WinForms), and
    while it is outstanding every other UIA call to that app queues behind it (measured: 65 s per
    call). For Win32-backed buttons, send BM_CLICK from a worker thread instead: pure Win32, so
    only that thread waits. Other controls use UIA Invoke.
    """
    h = getattr(c, "handle", None)
    if h and c.element_info.control_type == "Button":
        th = threading.Thread(target=lambda: user32.SendMessageW(h, BM_CLICK, 0, 0), daemon=True)
        th.start()
        th.join(0.3)
        return
    c.invoke()


def _window_step(app: "App", do: str, val, t: float) -> None:
    deadline = time.time() + t
    while time.time() < deadline:
        wins = [w for w in app.windows() if re.search(str(val), w.window_text() or "")]
        if wins:
            if do == "close_window":
                user32.PostMessageW(wins[0].handle, WM_CLOSE, 0, 0)
            return
        time.sleep(0.1)
    raise RuntimeError(f"no window matching /{val}/ within {t:.0f}s")


def _toggle_to(c, want) -> None:
    if want is None or c.get_toggle_state() != want:
        c.toggle()


def _expect_text(c, val, t: float) -> None:
    deadline = time.time() + t
    text = control_text(c)
    while str(val) not in text and time.time() < deadline:
        time.sleep(0.1)
        text = control_text(c)
    if str(val) not in text:
        raise RuntimeError(f'expected text containing "{val}", got "{text[:120]}"')


CONTROL_ACTIONS = {
    "wait_for": lambda c, v, t: None,
    "click": lambda c, v, t: click_async(c),
    "fill": lambda c, v, t: c.set_edit_text(str(v or "")),
    "select": lambda c, v, t: c.select(str(v)),
    "check": lambda c, v, t: _toggle_to(c, 1),
    "uncheck": lambda c, v, t: _toggle_to(c, 0),
    "toggle": lambda c, v, t: _toggle_to(c, None),
    "press": lambda c, v, t: c.type_keys(str(v), set_foreground=True),
    "expect_text": _expect_text,
}


def run_step(app: "App", step: dict, out_dir: Path, shot_name: str) -> str | None:
    do = step.get("do")
    t = step.get("timeout_ms", 5000) / 1000
    val = step.get("value")
    if do == "wait":
        time.sleep(float(val or 500) / 1000)
        return None
    if do in ("expect_window", "close_window"):
        _window_step(app, do, val, t)
        return None
    if do == "screenshot":
        wins = app.windows()
        if not wins:
            raise RuntimeError("no window to capture")
        return shot(wins[0], out_dir / f"{shot_name}.png")
    action = CONTROL_ACTIONS.get(do)
    if action is None:
        raise RuntimeError(f"unknown step '{do}'")
    target = step.get("target") or {}
    c = find_target(app, target, t)
    if c is None:
        raise RuntimeError(f"target not found within {t:.0f}s: {target}")
    action(c, val, t)
    return None


def flow_errors(app: "App", seen: set) -> list[str]:
    """New error-looking windows since the last check (recorded, then closed)."""
    errs = []
    for h in top_windows(tree_pids(app.pid)):
        if h in seen:
            continue
        seen.add(h)
        title, body = _text(h), " | ".join(child_texts(h))
        if ERROR_WORDS.search(title) or ERROR_WORDS.search(body):
            errs.append(f"error dialog: {title}: {body}"[:300])
            user32.PostMessageW(h, WM_CLOSE, 0, 0)
    return errs


def run_flow(inp: dict, out_dir: Path, fi: int, flow: dict) -> dict:
    fr = {"name": flow.get("name") or f"flow {fi + 1}", "passed": True, "steps": []}
    progress("flow", name=fr["name"], steps=len(flow.get("steps", [])))
    report = {"crashes": []}
    app, win = start_app(inp, out_dir, report)
    if win is None:
        fr.update(passed=False, steps=[{"i": 0, "do": "launch", "result": "fail", "error": report.get("error")}])
        return fr
    wait_idle(win.handle, inp.get("settle_ms", 3000) / 1000)
    seen = set(top_windows(tree_pids(app.pid)))
    for i, step in enumerate(flow.get("steps", []), 1):
        rec = {"i": i, "do": step.get("do"), "target": step.get("target"), "result": "ok"}
        if not fr["passed"] and not flow.get("continue_on_error"):
            rec["result"] = "skipped"
            fr["steps"].append(rec)
            continue
        t0 = time.time()
        try:
            p = run_step(app, step, out_dir, f"flow{fi + 1}-step{i}")
            if p:
                rec["screenshot"] = p
        except Exception as e:  # noqa: BLE001
            rec.update(result="fail", error=str(e).splitlines()[0][:300] if str(e) else type(e).__name__)
        if app.alive():
            wait_idle(win.handle, 2.0)
            if not responds(win.handle, inp.get("hang_timeout_s", 5)):
                rec.update(result="fail", error=f"UI not responding for {inp.get('hang_timeout_s', 5)}s")
            errs = flow_errors(app, seen)
            if errs:
                rec.update(result="fail", error=" | ".join(errs))
        else:
            rec.update(result="fail", error=f"app exited with code {app.proc.poll()}")
        rec["ms"] = int((time.time() - t0) * 1000)
        if rec["result"] == "fail":
            fr["passed"] = False
            if app.alive():
                rec["screenshot"] = shot(win, out_dir / f"flow{fi + 1}-fail{i}.png")
        fr["steps"].append(rec)
        progress("flow_step", flow=fr["name"], i=i, do=step.get("do"), result=rec["result"])
        if not app.alive():
            break
    fr["stderr_tail"] = app.stderr_tail()[-1000:]
    app.close()
    return fr


def settle_inert_or_crash(inp: dict, app: App, win, desc: dict, rec: dict, report: dict) -> bool:
    """Nothing visible changed: inert, unless the app already died."""
    if not app.alive():
        return mark_crash(app, rec, report, desc["name"])
    rec["result"] = "no_effect"
    return False


def mark_crash(app: App, rec: dict, report: dict, after: str) -> bool:
    code = app.proc.poll()
    rec.update(result="crash", errors=[f"process exited with code {code}"])
    report["crashes"].append({"after": after, "exit_code": code})
    return True


def attribute_late_exit(inp: dict, app: App, report: dict) -> None:
    """Some apps take seconds to die (runtime teardown). If the app is dead, or dies within
    crash_grace_s after a last action that looked inert, blame the most recent action."""
    last = next((r for r in reversed(report["interactions"]) if r["result"] in ("ok", "no_effect")), None)
    if last is None or report["crashes"]:
        return
    if app.alive() and last["result"] == "no_effect":
        deadline = time.time() + inp.get("crash_grace_s", 3)
        while app.alive() and time.time() < deadline:
            time.sleep(0.1)
    if not app.alive():
        mark_crash(app, last, report, last["element"]["name"])
        progress("interaction_update", name=last["element"]["name"], result="crash", exit_code=app.proc.poll())


def explore(inp: dict, app: App, win, initial: list, known: set, report: dict, out_dir: Path) -> None:
    """Exercise each control once from the main window; stops at a crash or hang."""
    actions = 0
    for desc in initial:
        if actions >= inp.get("max_actions", 60):
            break
        rec = {"element": desc, "action": "none", "result": "", "errors": []}
        if not app.alive():
            attribute_late_exit(inp, app, report)
        why = skip_reason(inp, app, desc)
        c = None if why else find_again(win, desc)
        if why or c is None:
            rec.update(result="skipped", detail=why or "control gone after earlier interaction")
            report["interactions"].append(rec)
            continue
        actions += 1
        before = signature(win)
        rec["result"] = "ok"
        dog, err = run_action(inp, app, win, c, rec, known)
        stop = classify(inp, app, dog, err, rec, report)
        opened = sum(1 for d in report["dialogs"] if d["after"] == desc["name"])
        if opened and rec["result"] == "ok":
            rec["detail"] = f"opened {opened} dialog(s) (closed by the tester)"
        elif not stop and rec["result"] == "ok" and rec["action"] == "invoke" and signature(win) == before:
            stop = settle_inert_or_crash(inp, app, win, desc, rec, report)
        if rec["result"] == "error_dialog":
            report["screenshots"] += [p for p in [shot(win, out_dir / f"error-{actions}.png")] if p]
        report["interactions"].append(rec)
        progress("interaction", n=actions, name=desc["name"], control=desc["control_type"], action=rec["action"],
                 result=rec["result"], ms=rec.get("duration_ms"))
        if stop:
            break



def soak_loop(inp: dict, app: App, win, soak: float, out_dir: Path, report: dict) -> float:
    """Watch a running Unity player for crashes, crash-handler launches and hangs; screenshot periodically."""
    import unity_mode as um

    t0, next_shot = time.time(), 0.0
    hung_since = None
    while time.time() - t0 < soak:
        el = time.time() - t0
        if not app.alive():
            report["crashes"].append({"after": f"soak {el:.1f}s", "exit_code": app.proc.poll()})
            break
        crash_win = um.crash_dialog_open(tree_pids(app.pid), top_windows, _text)
        if crash_win:
            report["crashes"].append({"after": f"soak {el:.1f}s", "exit_code": None, "detail": f"crash handler window: {crash_win}"})
            break
        if responds(win.handle, 1.0):
            hung_since = None
        else:
            hung_since = hung_since or time.time()
            if time.time() - hung_since >= inp.get("hang_timeout_s", 5):
                report["hangs"].append({"after": f"soak {el:.1f}s", "seconds": round(time.time() - hung_since, 1)})
                break
        if el >= next_shot:
            shot_path = shot(win, out_dir / f"unity-{int(el):03d}s.png")
            if shot_path:
                report["screenshots"].append(shot_path)
            next_shot = el + float(inp.get("shot_every_s", 5))
        progress("soak", t=round(el, 1), of=soak)
        time.sleep(0.5)
    return t0


def collect_frames(pm, pm_csv: Path, exe_pm, soak: float, t0: float) -> dict:
    import unity_mode as um

    if pm is None:
        return {"ok": False, "error": "PresentMon not found" if not exe_pm else "frame_timing disabled"}
    timed_out = False
    try:
        pm.wait(timeout=max(5, soak - (time.time() - t0) + 5))
    except subprocess.TimeoutExpired:
        timed_out = True
        pm.kill()
        pm.wait(5)  # never leave a PresentMon (and its ETW session) behind
    out = (pm.stdout.read() or b"").decode("utf-8", "replace") if pm.stdout else ""
    stats = um.frame_stats(pm_csv)
    if not stats.get("ok"):
        # Say what PresentMon said, not a guess: a missing CSV has several causes.
        if "access" in out.lower():
            stats["error"] = "PresentMon needs admin or 'Performance Log Users' membership"
        stats["presentmon"] = {"exit_code": pm.returncode, "timed_out": timed_out, "output": out.strip()[-600:]}
    return stats


def port_holder(url: str) -> str | None:
    """Who already listens on the ws:// URL's port, before we launch anything. A second instance
    cannot bind it, so the test would only wait out its timeouts talking to the wrong process."""
    m = re.match(r"wss?://[^/:]+:(\d+)", url or "")
    if not m:
        return None
    port = int(m.group(1))
    try:
        conns = psutil.net_connections(kind="tcp")
    except Exception:  # noqa: BLE001
        return None
    for c in conns:
        if c.status == psutil.CONN_LISTEN and c.laddr and c.laddr.port == port:
            try:
                proc = psutil.Process(c.pid)
                return f"port {port} held by PID {c.pid} ({proc.name()})"
            except Exception:  # noqa: BLE001
                # The owner exited but the socket lives on: a child inherited the listening handle.
                kids = [f"{k.pid} {k.name()}" for k in psutil.process_iter(["ppid"]) if k.info.get("ppid") == c.pid]
                return f"port {port} held by exited PID {c.pid}; inherited by orphaned child(ren): {', '.join(kids) or 'unknown'}"
    return None


def wait_until_ready(inp: dict, app: App, log_path: Path, events=None) -> dict:
    """Wait for the app to finish starting before the soak: until `ready_event` (regex) arrives on
    the app's event stream, `ready_log` (regex) appears in the player log, or `ready_title` (regex)
    matches a window title. Fails fast rather than waiting out `ready_timeout_s`: on a `fail_log`
    match, when `app_events` has not connected within `stall_s`, or when the app goes `stall_s`
    with no event and no log growth. Returns what happened."""
    rx_log = re.compile(inp["ready_log"]) if inp.get("ready_log") else None
    rx_title = re.compile(inp["ready_title"]) if inp.get("ready_title") else None
    rx_fail = re.compile(inp["fail_log"]) if inp.get("fail_log") else None
    ready_event = inp.get("ready_event")
    if not (rx_log or rx_title or ready_event):
        return {"waited_s": 0, "matched": None}
    t0 = time.time()
    limit = float(inp.get("ready_timeout_s", 120))
    stall = float(inp.get("stall_s", 15) or 0)
    use_events = bool(ready_event and events is not None)
    last_size, last_activity, last_count = -1, t0, 0

    def elapsed() -> float:
        return round(time.time() - t0, 1)

    while time.time() - t0 < limit and app.alive():
        if use_events:
            got = events.wait_for(ready_event, 1.0)
            if got is not None:
                return {"waited_s": round(time.time() - t0, 2), "matched": "event", "event": got[:300]}
        text = None
        size = log_path.stat().st_size if log_path.exists() else 0
        if size != last_size:
            last_size, last_activity = size, time.time()
            text = log_path.read_text(encoding="utf-8", errors="replace") if size else ""
            if rx_fail and (hit := rx_fail.search(text)):
                return {"waited_s": elapsed(), "matched": None, "failed": f"fail_log matched: {hit.group(0)[:200]}"}
            if rx_log and rx_log.search(text):
                return {"waited_s": elapsed(), "matched": "log"}
        if rx_title and any(rx_title.search(_text(h)) for h in top_windows(tree_pids(app.pid))):
            return {"waited_s": elapsed(), "matched": "title"}
        if events is not None and events.count != last_count:
            last_count, last_activity = events.count, time.time()
        if stall > 0:
            if events is not None and not events.connected and time.time() - t0 > stall:
                return {"waited_s": elapsed(), "matched": None,
                        "stalled": f"app event stream not connected after {stall:.0f}s ({events.error or 'no answer'})"}
            if time.time() - last_activity > stall:
                return {"waited_s": elapsed(), "matched": None,
                        "stalled": f"no app event and no log output for {stall:.0f}s"}
        progress("waiting_ready", t=elapsed(), app_events=events.connected if events else None)
        if not use_events:
            time.sleep(0.5)
    return {"waited_s": elapsed(), "matched": None, "timed_out": app.alive()}


def unity_soak(inp: dict, out_dir: Path, report: dict) -> dict:
    """Unity player: no UIA controls to exercise, so launch windowed, run input steps, and soak."""
    import unity_mode as um

    args, log_path = um.unity_args(inp.get("args", []), out_dir, inp.get("width", 1280), inp.get("height", 720))
    uinp = {**inp, "args": args}
    soak = float(inp.get("soak_s", 20))
    u = {"log_path": str(log_path), "soak_s": soak, "steps": []}
    report["unity"] = u
    busy = port_holder(inp.get("app_events") or "")
    if busy:
        u["preflight"] = {"port_busy": busy}
        progress("preflight_failed", port_busy=busy)
        return report
    progress("launching", exe=inp.get("exe"), unity=True)
    app, win = start_app(uinp, out_dir, report)
    # Connect while the app starts: the client retries until the app's server is up.
    events = um.AppEvents(inp["app_events"], progress, float(inp.get("ready_timeout_s", 120))) if inp.get("app_events") else None
    if win is None:
        u["log"] = um.parse_player_log(log_path.read_text(encoding="utf-8", errors="replace")) if log_path.exists() else None
        if events:
            u["app_events"] = events.summary()
            events.close()
        return report
    ready = wait_until_ready(inp, app, log_path, events)
    u["ready"] = ready
    if ready.get("failed") or ready.get("stalled"):
        # Fail fast: no steps, no soak on an app that never came up.
        progress("ready_failed", **{k: ready[k] for k in ("failed", "stalled") if k in ready})
        if (stalled_shot := shot(win, out_dir / "unity-stalled.png")):
            report["screenshots"].append(stalled_shot)
        if events:
            u["app_events"] = events.summary()
            events.close()
        report["exit_code"] = app.proc.poll()
        app.close()
        report["orphans"] = getattr(app, "orphans", [])
        u["log"] = um.parse_player_log(log_path.read_text(encoding="utf-8", errors="replace")) if log_path.exists() else None
        return report
    progress("window_ready", title=win.window_text(), pid=app.pid, unity=True, ready=ready)
    report["windows"] = [_text(h) for h in top_windows(tree_pids(app.pid))]
    pm, pm_csv = None, out_dir / "frames.csv"
    exe_pm = um.presentmon_exe(inp.get("tools_bin"))
    if inp.get("frame_timing", True) and exe_pm:
        pm = um.start_presentmon(exe_pm, Path(inp["exe"]).name, int(soak), pm_csv)
    # await_event matches events from the start of the previous step on (the ready point for the
    # first step), or, after a send, from just after that command's reply: "this happened because
    # of what we just did", never a stale event from boot or from before the command ran.
    previous_mark = events.mark() if events else 0
    for i, step in enumerate(inp.get("unity_steps") or [], 1):
        rec = {"i": i, "do": step.get("do"), "result": "ok"}
        step_mark = events.mark() if events else 0
        if step.get("do") == "await_event" and "since" not in step:
            step = {**step, "since": previous_mark}
        previous_mark = step_mark
        if step.get("value") is not None and step.get("do") == "send":
            rec["value"] = str(step["value"])[:120]
        try:
            if step.get("do") in ("send", "await_event"):
                rec.update(um.run_event_step(events, step))
                if "after" in rec:
                    previous_mark = rec.pop("after")
            else:
                um.post_input(user32, win.handle, step)
        except Exception as e:  # noqa: BLE001
            rec.update(result="fail", error=str(e)[:300])
        progress("step", i=i, do=rec["do"], result=rec["result"], ms=rec.get("ms"))
        u["steps"].append(rec)
    t0 = soak_loop(inp, app, win, soak, out_dir, report)
    if events:
        u["app_events"] = events.summary()
        events.close()
    u["frames"] = collect_frames(pm, pm_csv, exe_pm, soak, t0)
    # A crash is an exit before the close; read after it too, so a clean run reports its code
    # rather than null for "still running".
    exited_early = app.proc.poll()
    app.close()
    report["exit_code"] = app.proc.poll()
    report["exited_before_close"] = exited_early is not None
    report["orphans"] = getattr(app, "orphans", [])
    report["close_timed_out"] = getattr(app, "close_timed_out", False)
    report["close_ms"] = getattr(app, "close_ms", None)
    report["exit_ms"] = getattr(app, "exit_ms", None)
    report["exit_timed_out"] = getattr(app, "exit_timed_out", False)
    time.sleep(0.5)  # let the player flush its log
    u["log"] = um.parse_player_log(log_path.read_text(encoding="utf-8", errors="replace")) if log_path.exists() else None
    progress("done", unity=True, crashes=len(report["crashes"]), hangs=len(report["hangs"]))
    return report


def desktop_test(inp: dict) -> dict:
    if sys.platform != "win32":
        return {"ok": False, "error": "desktop_test requires Windows (UI Automation)"}
    out_dir = Path(inp.get("out_dir") or "out")
    out_dir.mkdir(parents=True, exist_ok=True)
    report = {
        "ok": True, "app": inp.get("exe"), "pid": None, "windows": [], "controls_found": 0,
        "interactions": [], "crashes": [], "hangs": [], "error_dialogs": [], "dialogs": [],
        "persistence": [], "screenshots": [], "stderr_tail": "", "exit_code": None,
    }
    import unity_mode

    if inp.get("unity") or (inp.get("unity") is None and unity_mode.is_unity_exe(inp.get("exe", ""))):
        return unity_soak(inp, out_dir, report)
    report["flows"] = [run_flow(inp, out_dir, i, f) for i, f in enumerate(inp.get("flows") or [])]
    if inp.get("explore") is False:
        report["persistence"] = check_persistence(inp, out_dir) if inp.get("persist") else []
        return report
    progress("launching", exe=inp.get("exe"))
    app, win = start_app(inp, out_dir, report)
    if win is None:
        progress("failed", error=report.get("error"))
        return report
    progress("window_ready", title=win.window_text(), pid=app.pid, waited_s=round(wait_idle(win.handle, inp.get("settle_ms", 3000) / 1000), 2))
    report["windows"] = [w.window_text() for w in app.windows()]
    report["screenshots"] += [p for p in [shot(win, out_dir / "start.png")] if p]
    known = {w.handle for w in app.windows()}
    persist_checks = inp.get("persist") or []
    expected = set_persisted(win, persist_checks)  # set now; verified after one relaunch
    initial = [d for d in (describe(c) for c in candidates(win)) if not any(match_desc(d, chk) for chk in persist_checks)]
    report["controls_found"] = len(initial)
    progress("controls", count=len(initial), persist=len(persist_checks))
    explore(inp, app, win, initial, known, report, out_dir)
    attribute_late_exit(inp, app, report)
    report["stderr_tail"] = app.stderr_tail()
    survived = app.alive()
    report["exited_before_close"] = not survived
    if survived:
        app.close()  # graceful close, so the app can save its state
    report["exit_code"] = app.proc.poll() if app.proc else None
    for key in ("close_ms", "exit_ms", "close_timed_out", "exit_timed_out"):
        report[key] = getattr(app, key, None)
    if persist_checks:
        progress("persistence", relaunches=1 if survived else 2)
        # one relaunch when the exploration run ended cleanly; otherwise a fresh set/verify cycle
        report["persistence"] = verify_persisted(inp, out_dir, expected) if survived else check_persistence(inp, out_dir)
    progress("done", interactions=len(report["interactions"]), crashes=len(report["crashes"]), hangs=len(report["hangs"]))
    return report


def match_desc(d: dict, chk: dict) -> bool:
    if chk.get("auto_id") and d["auto_id"] != chk["auto_id"]:
        return False
    if chk.get("name") and d["name"] != chk["name"]:
        return False
    return bool(chk.get("auto_id") or chk.get("name"))


def set_persisted(win, checks: list[dict]) -> dict:
    """Set each persisted control to a recognizable value. Returns {json(check): expected or error}."""
    expected = {}
    for chk in checks:
        key = json.dumps(chk, sort_keys=True)
        c = next((d for d in candidates(win) if match(d, chk)), None)
        if c is None:
            expected[key] = {"error": "control not found"}
            continue
        try:
            if c.element_info.control_type == "CheckBox":
                c.toggle()
                expected[key] = {"value": str(c.get_toggle_state())}
            else:
                v = f"kritic-persist-{int(time.time()) % 100000}"
                c.set_edit_text(v)
                expected[key] = {"value": v}
        except Exception as e:  # noqa: BLE001
            expected[key] = {"error": f"could not set: {str(e)[:120]}"}
    return expected


def verify_persisted(inp: dict, out_dir: Path, expected: dict) -> list[dict]:
    app = App(inp, out_dir)
    app.stderr_path = out_dir / "stderr-relaunch.txt"
    app.launch()
    win = app.main_window(inp.get("startup_timeout_s", 30))
    results = []
    if win is not None:
        wait_idle(win.handle, inp.get("settle_ms", 3000) / 1000)
    for key, exp in expected.items():
        chk = json.loads(key)
        if "error" in exp:
            results.append({**chk, "persisted": False, "error": exp["error"]})
            continue
        actual = None
        if win is not None:
            c = next((d for d in candidates(win) if match(d, chk)), None)
            if c is not None:
                actual = str(c.get_toggle_state()) if c.element_info.control_type == "CheckBox" else c.get_value()
        results.append({**chk, "expected": exp["value"], "actual": actual, "persisted": actual == exp["value"]})
    app.close()
    return results


def check_persistence(inp: dict, out_dir: Path) -> list[dict]:
    """Launch fresh, set each control, close normally; launch again and read the values back."""
    checks = inp.get("persist") or []
    if not checks:
        return []
    results = []
    a = App(inp, out_dir)
    a.stderr_path = out_dir / "stderr-persist-1.txt"
    a.launch()
    w = a.main_window(inp.get("startup_timeout_s", 30))
    expected = {}
    if w is None:
        a.close()
        return [{**c, "persisted": False, "error": "app did not start for persistence check"} for c in checks]
    time.sleep(inp.get("settle_ms", 800) / 1000)
    for chk in checks:
        c = next((d for d in candidates(w) if match(d, chk)), None)
        if c is None:
            results.append({**chk, "persisted": False, "error": "control not found"})
            continue
        try:
            if c.element_info.control_type == "CheckBox":
                c.toggle()
                expected[json.dumps(chk)] = str(c.get_toggle_state())
            else:
                v = f"kritic-persist-{int(time.time()) % 100000}"
                c.set_edit_text(v)
                expected[json.dumps(chk)] = v
        except Exception as e:  # noqa: BLE001
            results.append({**chk, "persisted": False, "error": f"could not set: {str(e)[:120]}"})
    time.sleep(inp.get("wait_ms", 400) / 1000)
    a.close()
    b = App(inp, out_dir)
    b.stderr_path = out_dir / "stderr-persist-2.txt"
    b.launch()
    w2 = b.main_window(inp.get("startup_timeout_s", 30))
    if w2 is not None:
        time.sleep(inp.get("settle_ms", 800) / 1000)
    for k, v in expected.items():
        chk = json.loads(k)
        actual = None
        if w2 is not None:
            c = next((d for d in candidates(w2) if match(d, chk)), None)
            if c is not None:
                actual = str(c.get_toggle_state()) if c.element_info.control_type == "CheckBox" else c.get_value()
        results.append({**chk, "expected": v, "actual": actual, "persisted": actual == v})
    b.close()
    return results




def match(c, chk: dict) -> bool:
    d = describe(c)
    if chk.get("auto_id") and d["auto_id"] != chk["auto_id"]:
        return False
    if chk.get("name") and d["name"] != chk["name"]:
        return False
    return bool(chk.get("auto_id") or chk.get("name"))


# --------------------------------------------------------------------------- resource sampler


def gpu_sample() -> dict | None:
    exe = shutil.which("nvidia-smi")
    if not exe:
        return None
    try:
        out = subprocess.run(
            [exe, "--query-gpu=utilization.gpu,memory.used", "--format=csv,noheader,nounits"],
            capture_output=True, text=True, timeout=3,
        ).stdout.strip().splitlines()[0]
        util, mem = (float(x) for x in out.split(","))
        return {"util": util, "mem_mb": mem}
    except Exception:
        return None


def slope(xs: list[float], ys: list[float]) -> float:
    n = len(xs)
    if n < 2:
        return 0.0
    mx, my = sum(xs) / n, sum(ys) / n
    den = sum((x - mx) ** 2 for x in xs)
    return sum((x - mx) * (y - my) for x, y in zip(xs, ys, strict=True)) / den if den else 0.0


def sample_tree(root: int, procs: dict) -> dict:
    """One sample summed over the process tree. `procs` caches psutil handles (cpu_percent needs
    a primed handle per process)."""
    pids = tree_pids(root)
    tot = {"cpu": 0.0, "rss": 0.0, "priv": 0.0, "handles": 0.0, "threads": 0.0, "read": 0.0, "write": 0.0, "procs": len(pids)}
    for pid in pids:
        p = procs.get(pid)
        if p is None:
            try:
                p = psutil.Process(pid)
                p.cpu_percent(None)  # prime
                procs[pid] = p
            except psutil.Error:
                continue
        try:
            with p.oneshot():
                mi = p.memory_info()
                io = p.io_counters()
                tot["cpu"] += p.cpu_percent(None)
                tot["rss"] += mi.rss
                tot["priv"] += getattr(mi, "private", mi.rss)
                tot["threads"] += p.num_threads()
                tot["handles"] += getattr(p, "num_handles", lambda: 0)()
                tot["read"] += io.read_bytes
                tot["write"] += io.write_bytes
        except psutil.Error:
            continue
    return tot


def summarize(samples: list[dict], gpu: list[dict], exit_code) -> dict:
    mb = 1024 * 1024
    ts = [s["t"] for s in samples]
    cpus = sorted(s["cpu"] for s in samples)
    rss = [s["rss"] / mb for s in samples]
    span = max(ts[-1] - ts[0], 1e-6)
    first, last = samples[0], samples[-1]
    ncpu = psutil.cpu_count() or 1
    return {
        "duration_s": round(ts[-1], 1),
        "samples": len(samples),
        "cpu_cores": ncpu,
        "cpu_avg_pct": round(sum(cpus) / len(cpus), 1),
        "cpu_p95_pct": round(cpus[int(0.95 * (len(cpus) - 1))], 1),
        "cpu_max_pct": round(cpus[-1], 1),
        "cpu_avg_pct_of_machine": round(sum(cpus) / len(cpus) / ncpu, 1),
        "rss_start_mb": round(rss[0], 1),
        "rss_end_mb": round(rss[-1], 1),
        "rss_peak_mb": round(max(rss), 1),
        "rss_slope_mb_per_min": round(slope(ts, rss) * 60, 2),
        "private_end_mb": round(last["priv"] / mb, 1),
        "handles_start": int(first["handles"]),
        "handles_end": int(last["handles"]),
        "threads_peak": int(max(s["threads"] for s in samples)),
        "procs_peak": int(max(s["procs"] for s in samples)),
        "read_mb": round((last["read"] - first["read"]) / mb, 2),
        "write_mb": round((last["write"] - first["write"]) / mb, 2),
        "read_mb_per_s": round((last["read"] - first["read"]) / mb / span, 3),
        "write_mb_per_s": round((last["write"] - first["write"]) / mb / span, 3),
        "gpu_util_avg_pct": round(sum(g["util"] for g in gpu) / len(gpu), 1) if gpu else None,
        "gpu_util_max_pct": max((g["util"] for g in gpu), default=None),
        "gpu_mem_delta_mb": round(gpu[-1]["mem_mb"] - gpu[0]["mem_mb"], 0) if len(gpu) > 1 else None,
        "exited_early": exit_code is not None,
        "exit_code": exit_code,
    }


def start_target(inp: dict):
    """Returns (root_pid, launched_popen_or_None, error)."""
    if inp.get("pid"):
        if not inp.get("allow_attach"):
            return None, None, "attaching to an existing pid requires allow_attach"
        return int(inp["pid"]), None, None
    cmd = inp.get("command")
    if not cmd:
        return None, None, "give command or pid"
    # A string command is run through the shell on purpose: resource_critic measures whatever
    # command the caller (Claude, on the user's behalf) gives it, like artifact_validate's
    # `commands`. Pass an argv list to avoid the shell.
    proc = subprocess.Popen(  # noqa: S602  # nosec B602 - caller-supplied command, by design
        cmd, shell=isinstance(cmd, str), cwd=inp.get("cwd") or None,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL,
    )
    return proc.pid, proc, None


def resource(inp: dict) -> dict:
    duration = float(inp.get("duration_s", 20))
    interval = float(inp.get("interval_ms", 500)) / 1000
    warm = float(inp.get("warmup_s", 1))
    root, launched, err = start_target(inp)
    if err:
        return {"ok": False, "error": err}
    procs: dict[int, psutil.Process] = {}
    samples, gpu, exit_code = [], [], None
    t0 = time.time()
    while time.time() - t0 < duration + warm:
        tot = sample_tree(root, procs)
        t = time.time() - t0
        if t >= warm:
            samples.append({"t": round(t - warm, 2), **tot})
            g = gpu_sample() if len(samples) % max(1, int(1 / interval)) == 0 else None
            if g:
                gpu.append(g)
        if launched is not None and launched.poll() is not None and not tree_pids(root) - {root}:
            exit_code = launched.poll()
            break
        time.sleep(interval)
    if launched is not None and launched.poll() is None:
        for pid in tree_pids(root):
            try:
                psutil.Process(pid).kill()
            except psutil.Error:
                pass
    if not samples:
        return {"ok": False, "error": "no samples (process exited immediately?)", "exit_code": exit_code}
    step = max(1, len(samples) // 60)
    timeline = [{"t": s["t"], "cpu": round(s["cpu"], 1), "rss_mb": round(s["rss"] / 1048576, 1)} for s in samples[::step]]
    return {"ok": True, "stats": summarize(samples, gpu, exit_code), "timeline": timeline,
            "gpu_note": "whole-GPU (nvidia-smi), not per-process"}


def main() -> None:
    try:
        # Bytes, decoded as UTF-8: sys.stdin uses the Windows code page (cp1252), which turned
        # "Item Lyrics…" into "Item Lyricsâ€¦" and broke every non-ASCII target.
        inp = json.loads(sys.stdin.buffer.read().decode("utf-8"))
    except Exception as e:
        emit({"ok": False, "error": f"bad input JSON: {e}"})
        return
    try:
        import hygiene

        if inp.get("mode") == "hygiene":
            emit(hygiene.audit(inp))
            return
        # Whatever the run starts must be gone when it ends: snapshot, run, then report leaks.
        roots = hygiene.roots_for(inp)
        before = hygiene.snapshot(roots) if roots else None
        result = resource(inp) if inp.get("mode") == "resource" else desktop_test(inp)
        if before is not None and isinstance(result, dict):
            result["hygiene"] = hygiene.leaks(before, roots)
        emit(result)
    except Exception as e:
        emit({"ok": False, "error": f"harness crashed: {type(e).__name__}: {str(e)[:300]}"})


if __name__ == "__main__":
    main()

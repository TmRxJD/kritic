"""Process and resource hygiene: what a project (or a test run) leaves behind.

Facts only, no model. Three uses:
- `audit(inp)` (mode "hygiene", the hygiene_critic tool): the machine as it is now. Processes whose
  executable or command line sits under a project root, which of them are orphans (parent gone),
  listening sockets still bound after their owner exited (a child inherited the handle), and
  leftover files matching `leftover_globs`. `clean=true` kills orphans and socket inheritors;
  files are only ever reported.
- `snapshot(roots)` before a test launches the app, then `leaks(before, roots)` after it closed:
  processes and listening ports the run created that are still alive.
"""

from __future__ import annotations

import glob
import os
import time
from pathlib import Path

import psutil

MAX_ITEMS = 50


def _norm(p: str) -> str:
    return os.path.normcase(os.path.abspath(p)).rstrip("\\/")


def _under(path: str | None, roots: list[str]) -> bool:
    if not path:
        return False
    p = _norm(path)
    return any(p == r or p.startswith(r + os.sep) for r in roots)


def _mentions(cmdline: list[str], roots: list[str]) -> bool:
    for arg in cmdline or []:
        arg = arg.strip('"')
        if (os.path.isabs(arg) or arg[1:3] in (":\\", ":/")) and _under(arg, roots):
            return True
    return False


def _parent_alive(proc: psutil.Process) -> bool:
    """A parent counts only if it is the same process that started this one (PIDs get reused)."""
    try:
        parent = psutil.Process(proc.ppid())
        return parent.create_time() <= proc.create_time()
    except psutil.Error:
        return False


def _listeners() -> list[tuple[int, int]]:
    """(port, pid) of every listening TCP socket."""
    try:
        return [(c.laddr.port, c.pid) for c in psutil.net_connections(kind="tcp")
                if c.status == psutil.CONN_LISTEN and c.laddr and c.pid]
    except psutil.Error:
        return []


def _describe(proc: psutil.Process, ports: dict[int, list[int]]) -> dict:
    try:
        cmd = " ".join(proc.cmdline())
    except psutil.Error:
        cmd = ""
    return {
        "pid": proc.pid,
        "ppid": proc.ppid(),
        "name": proc.name(),
        "cmd": cmd[:200],
        "age_s": round(time.time() - proc.create_time()),
        "ports": ports.get(proc.pid, []),
    }


def project_processes(roots: list[str], exclude: set[int] = frozenset()) -> list[psutil.Process]:
    roots = [_norm(r) for r in roots]
    found = []
    for proc in psutil.process_iter():
        if proc.pid in exclude or proc.pid == os.getpid():
            continue
        try:
            exe = proc.exe()
        except psutil.Error:
            exe = None
        try:
            if _under(exe, roots) or _mentions(proc.cmdline(), roots):
                found.append(proc)
        except psutil.Error:
            continue
    return found


def _ports_by_pid() -> dict[int, list[int]]:
    out: dict[int, list[int]] = {}
    for port, pid in _listeners():
        out.setdefault(pid, []).append(port)
    return out


def dead_owner_sockets() -> list[dict]:
    """Listening sockets whose owning process has exited: some child inherited the handle and keeps
    the port bound. Returns the port, the dead owner and the children it left behind."""
    out = []
    for port, pid in _listeners():
        if psutil.pid_exists(pid):
            try:
                psutil.Process(pid)
                continue
            except psutil.Error:
                pass
        heirs = []
        for k in psutil.process_iter(["ppid", "name"]):
            if k.info.get("ppid") == pid:
                heirs.append({"pid": k.pid, "name": k.info.get("name")})
        out.append({"port": port, "owner_pid": pid, "inheritors": heirs})
    return out


def audit(inp: dict) -> dict:
    roots = [r for r in inp.get("roots") or [] if r]
    if not roots:
        return {"ok": False, "error": "hygiene: no roots given"}
    ignore = {n.lower() for n in inp.get("ignore_names") or []}
    clean = bool(inp.get("clean"))
    ports = _ports_by_pid()

    procs, orphans = [], []
    for proc in project_processes(roots):
        try:
            if proc.name().lower() in ignore:
                continue
            d = _describe(proc, ports)
            d["orphan"] = not _parent_alive(proc)
            procs.append(d)
            if d["orphan"]:
                orphans.append(proc)
        except psutil.Error:
            continue

    sockets = dead_owner_sockets()
    killed = []
    if clean:
        targets = {p.pid: p for p in orphans}
        for s in sockets:
            for h in s["inheritors"]:
                try:
                    targets.setdefault(h["pid"], psutil.Process(h["pid"]))
                except psutil.Error:
                    pass
        for pid, proc in targets.items():
            try:
                name = proc.name()
                proc.kill()
                proc.wait(3)
                killed.append(f"{pid} {name}")
            except psutil.Error:
                pass

    leftovers = []
    for pattern in inp.get("leftover_globs") or []:
        leftovers.extend(glob.glob(os.path.expandvars(os.path.expanduser(pattern))))

    return {
        "ok": True,
        "roots": roots,
        "processes": procs[:MAX_ITEMS],
        "orphans": [p for p in procs if p["orphan"]][:MAX_ITEMS],
        "dead_owner_sockets": sockets[:MAX_ITEMS],
        "leftovers": sorted(set(leftovers))[:MAX_ITEMS],
        "killed": killed,
        "cleaned": clean,
    }


# ------------------------------------------------------------------ around a test run

def snapshot(roots: list[str]) -> dict:
    return {
        "pids": {p.pid for p in project_processes(roots)},
        "listen": set(_listeners()),
        "t": time.time(),
    }


def leaks(before: dict, roots: list[str], kill: bool = True) -> dict:
    """After the app under test has closed: processes under `roots` and listening ports that the run
    created and that are still alive. Killed when `kill`, always reported."""
    ports = _ports_by_pid()
    leaked = []
    for proc in project_processes(roots, exclude=before["pids"]):
        try:
            if proc.create_time() < before["t"] - 1:
                continue
            leaked.append((proc, _describe(proc, ports)))
        except psutil.Error:
            continue
    new_ports = []
    for port, pid in set(_listeners()) - before["listen"]:
        try:
            name = psutil.Process(pid).name()
        except psutil.Error:
            name = "exited owner (inherited handle)"
        # Only ports the run owns: held by a leaked process, or by an owner that exited.
        if any(d["pid"] == pid for _, d in leaked) or not psutil.pid_exists(pid):
            new_ports.append({"port": port, "pid": pid, "name": name})
    if kill:
        for proc, _ in leaked:
            try:
                proc.kill()
            except psutil.Error:
                pass
    return {"processes": [d for _, d in leaked][:MAX_ITEMS], "ports": new_ports[:MAX_ITEMS]}


def roots_for(inp: dict) -> list[str]:
    """What a desktop run owns: the app's directory, its cwd, and any extra `hygiene_roots`."""
    roots = [str(Path(inp["exe"]).parent)] if inp.get("exe") else []
    command = inp.get("command")
    first = command[0] if isinstance(command, list) and command else None
    if isinstance(command, str) and command.strip():
        first = command.strip().split('"')[1] if command.strip().startswith('"') else command.split()[0]
    if first and os.path.isabs(first):
        roots.append(str(Path(first).parent))
    if inp.get("cwd"):
        roots.append(inp["cwd"])
    roots.extend(inp.get("hygiene_roots") or [])
    return roots

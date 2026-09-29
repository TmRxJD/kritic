"""Live tests for hygiene.py: reproduce an inherited listening socket and an orphan, then audit.
uv run --extra test pytest -q tests/test_hygiene.py
"""
import socket
import subprocess
import sys
import time
from pathlib import Path

import psutil
import pytest

sys.path.insert(0, str(Path(__file__).parent.parent))
import hygiene  # noqa: E402

# The parent opens an inheritable listening socket, starts a child that inherits it, and exits:
# the port stays bound by an exited owner, which is exactly what a real Unity app's engine did.
PARENT = r'''
import socket, subprocess, sys
s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(); s.set_inheritable(True)
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)", sys.argv[1]], close_fds=False)
open(sys.argv[2], "w").write(f"{s.getsockname()[1]} {child.pid}")
'''


@pytest.mark.skipif(sys.platform != "win32", reason="Windows handle inheritance")
def test_audit_finds_inherited_socket_and_orphan_then_cleans(tmp_path):
    marker = str(tmp_path / "marker")  # puts the child's command line under the audited root
    # Results go through a file: an inherited stdout pipe would never reach EOF (the same bug).
    result = tmp_path / "result.txt"
    subprocess.run([sys.executable, "-c", PARENT, marker, str(result)], stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30, check=True)
    port, child = (int(x) for x in result.read_text().split())
    try:
        deadline = time.time() + 10
        while time.time() < deadline and not any(s["port"] == port for s in hygiene.dead_owner_sockets()):
            time.sleep(0.2)
        audit = hygiene.audit({"roots": [str(tmp_path)]})
        sock = next(s for s in audit["dead_owner_sockets"] if s["port"] == port)
        assert any(h["pid"] == child for h in sock["inheritors"]), sock
        assert any(o["pid"] == child for o in audit["orphans"]), audit["orphans"]

        cleaned = hygiene.audit({"roots": [str(tmp_path)], "clean": True})
        assert any(k.startswith(f"{child} ") for k in cleaned["killed"]), cleaned["killed"]
        assert not psutil.pid_exists(child) or psutil.Process(child).status() == psutil.STATUS_ZOMBIE
        probe = socket.socket()
        probe.bind(("127.0.0.1", port))  # the port is free again
        probe.close()
    finally:
        try:
            psutil.Process(child).kill()
        except psutil.Error:
            pass


def test_leaks_reports_process_started_during_run(tmp_path):
    before = hygiene.snapshot([str(tmp_path)])
    proc = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)", str(tmp_path / "x")])
    try:
        time.sleep(0.5)
        leaked = hygiene.leaks(before, [str(tmp_path)])
        # A venv python.exe is a launcher that starts the real interpreter as its child: both leak.
        assert proc.pid in [p["pid"] for p in leaked["processes"]], leaked
        proc.wait(5)  # killed by leaks()
    finally:
        if proc.poll() is None:
            proc.kill()


def test_roots_for_command_and_exe():
    roots = hygiene.roots_for({"exe": "C:/a/b/app.exe", "command": '"C:/c/d/tool.exe" --x', "cwd": "C:/e"})
    assert [Path(r) for r in roots] == [Path("C:/a/b"), Path("C:/c/d"), Path("C:/e")]

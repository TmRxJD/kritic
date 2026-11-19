"""Bounded, shell-free repository inspection shared by all harness adapters."""
import hashlib
import os
import ntpath
from pathlib import Path
import re
import shutil
import subprocess  # nosec B404
import threading


def local_path(value):
    if not isinstance(value, (str, Path)):
        raise ValueError('Expected a local filesystem path')
    raw = str(value)
    if not raw or len(raw) > 4096 or any(ord(c) < 32 for c in raw):
        raise ValueError('Invalid or oversized filesystem path')
    drive, tail = ntpath.splitdrive(raw.replace('/', '\\'))
    if raw.replace('/', '\\').startswith('\\\\') or ':' in tail:
        raise ValueError('Network, device and alternate-stream paths are unsupported')
    if drive and (not re.fullmatch('[A-Za-z]:', drive) or not tail.startswith('\\')):
        raise ValueError('Drive-relative or device path is unsupported')
    return Path(raw)


def git(root, *args, allow_failure=False):
    env = dict(os.environ, GIT_OPTIONAL_LOCKS='0', GIT_TERMINAL_PROMPT='0', GIT_PAGER='cat', GIT_LITERAL_PATHSPECS='1')
    executable = shutil.which('git')
    if not executable:
        raise ValueError('Git executable was not found on PATH')
    # All subcommands/flags are supplied by this integration; paths follow --.
    process = subprocess.Popen(  # nosec B603
        [executable, '-c', 'core.fsmonitor=false', '-C', str(root), *args],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env,
        creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    timer = threading.Timer(10, process.kill)
    timer.daemon = True
    timer.start()
    try:
        output = process.stdout.read(1_000_001)
        if len(output) > 1_000_000:
            process.kill()
            raise ValueError('Git output exceeds 1 MB; narrow this evaluation')
        process.wait(timeout=2)
        if process.returncode and not allow_failure:
            raise ValueError(output.decode('utf8', errors='replace')[:1500])
        return None if process.returncode else output.decode('utf8')
    finally:
        timer.cancel()
        if process.poll() is None:
            process.kill()
        process.wait()
        process.stdout.close()


def identity(path):
    value = str(local_path(path).resolve()).replace('\\', '/')
    match = re.match(r'^/mnt/([a-z])/(.*)$', value)
    if match:
        value = match[1] + ':/' + match[2]
    return value.casefold() if re.match(r'^[a-zA-Z]:/', value) else value


def repository(path):
    path = local_path(path)
    if not path.is_absolute():
        raise ValueError('worktree must be an absolute path')
    top = git(path, 'rev-parse', '--path-format=absolute', '--show-toplevel').strip()
    common = git(path, 'rev-parse', '--path-format=absolute', '--git-common-dir').strip()
    head = git(path, 'rev-parse', '--verify', 'HEAD').strip()
    return {'repo': hashlib.sha256(identity(common).encode()).hexdigest(),
            'worktree': str(Path(top).resolve()), 'common_dir': common, 'head': head}


def relative_path(value):
    if not isinstance(value, str) or not value or len(value) > 1000 or '\\' in value:
        raise ValueError('scope paths require nonempty repository-relative forward-slash paths')
    directory = value.endswith('/')
    pieces = value.rstrip('/').split('/')
    if any(p in ('', '.', '..') or p.casefold() == '.git' for p in pieces) or ':' in value or any(ord(c) < 32 for c in value):
        raise ValueError('scope paths cannot escape the worktree or claim Git metadata')
    return '/'.join(pieces) + ('/' if directory else '')


def overlaps(left, right, case_insensitive=None):
    if case_insensitive is None:
        case_insensitive = os.name == 'nt'
    if case_insensitive:
        left, right = left.casefold(), right.casefold()
    left, right = left.rstrip('/'), right.rstrip('/')
    return left == right or left.startswith(right + '/') or right.startswith(left + '/')


def contains(root, scope, target):
    root = local_path(root).resolve()
    target = local_path(target)
    if not target.is_absolute():
        target = root / target
    target = target.resolve()
    try:
        relative = target.relative_to(root).as_posix()
    except ValueError:
        return False
    if re.match(r'^[a-zA-Z]:/', identity(root)):
        relative, scope = relative.casefold(), scope.casefold()
    if relative == '.git' or relative.startswith('.git/'):
        return False
    return relative == scope.rstrip('/') or (scope.endswith('/') and relative.startswith(scope))

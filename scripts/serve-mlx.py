#!/usr/bin/env python3
"""Write a validated launchd job for an explicit upstream Kev MLX profile.
Requires Python 3.12+ and an upstream checkout with `uv sync --extra serve --no-dev`.
Does not execute programs, install dependencies, or choose fallback models/backends.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import plistlib
import socket
import sys
import tomllib
from urllib.parse import urlsplit


def build_job(cfg, kev_dir, log_dir, ram):
    k = cfg['kev']
    if cfg['review']['backend'] != 'local' or k['local_runtime'] != 'mlx' or k.get('cpu_only', False):
        raise ValueError('select a local MLX profile with CPU fallback disabled')
    url = urlsplit(k['native_url'])
    if (url.scheme != 'http' or url.hostname != '127.0.0.1' or url.path not in ('', '/')
            or url.query or url.fragment or url.username or url.password):
        raise ValueError('launcher requires an HTTP 127.0.0.1 origin')
    port = url.port or 80
    run = k['native_run']
    if not run.startswith('jaredpalmer/kev-') or '@' not in run:
        raise ValueError('pin native_run to an explicit upstream Hub revision before launching')
    if k['native_dtype'] != 'bfloat16':
        raise ValueError('this launcher supports the upstream bf16 MLX path only')
    if 'kev-0.8b@' not in run and ram < 24 * 1024**3:
        raise ValueError('larger checkpoint requires at least 24 GiB; profile selection needs no weights')
    kev, logs = Path(kev_dir).resolve(), Path(log_dir).resolve()
    python = kev / '.venv/bin/python'
    if not python.is_file():
        raise ValueError(f'{python} missing; sync the upstream serve environment first')
    # launchd EnvironmentVariables augments its inherited environment. env -i is the
    # actual isolation boundary; arguments are fixed data, interpreted by no shell.
    environment = {
        'HOME': str(Path.home()), 'PATH': '/usr/bin:/bin:/usr/sbin:/sbin',
        'KEV_BACKEND': 'mlx', 'KEV_DTYPE': 'bf16', 'KEV_MERGE': '1',
        'KEV_TRUNCATE_STATES': '0', 'KEV_PREFIX_CACHE': '1',
        'KEV_PREFIX_MAX_TOKENS': str(k['max_seq_tokens']), 'PYTHONUNBUFFERED': '1',
    }
    return {
        'Label': f'com.kritic.kev-mlx.{port}',
        'ProgramArguments': ['/usr/bin/env', '-i',
                             *[f'{key}={value}' for key, value in sorted(environment.items())],
                             str(python), '-m', 'kev.serve', '--run', run, '--device', 'mps',
                             '--host', '127.0.0.1', '--port', str(port)],
        'WorkingDirectory': str(kev),
        'RunAtLoad': True, 'KeepAlive': False, 'ProcessType': 'Interactive',
        'StandardInPath': '/dev/null',
        'StandardOutPath': str(logs / 'kev-mlx.log'),
        'StandardErrorPath': str(logs / 'kev-mlx.log'),
    }


def write_job(job, log_dir):
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', int(job['ProgramArguments'][-1])))
    logs = Path(log_dir).resolve()
    logs.mkdir(parents=True, exist_ok=True)
    path = logs / 'kev-mlx.plist'
    # Never overwrite another launch job. Owner-only permissions, including during creation.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb') as target:
        plistlib.dump(job, target)
    return path


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config', required=True, type=Path)
    p.add_argument('--kev-dir', required=True, type=Path)
    p.add_argument('--log-dir', required=True, type=Path)
    a = p.parse_args()
    if sys.platform != 'darwin' or platform.machine() != 'arm64':
        p.error('native MLX serving requires Apple Silicon macOS')
    try:
        ram = os.sysconf('SC_PHYS_PAGES') * os.sysconf('SC_PAGE_SIZE')
        job = build_job(tomllib.loads(a.config.read_text()), a.kev_dir, a.log_dir, ram)
        path = write_job(job, a.log_dir)
    except (ValueError, OSError, KeyError) as exc:
        p.error(str(exc))
    print(json.dumps({'plist': str(path), 'label': job['Label'],
                      'bootstrap_argv': ['/bin/launchctl', 'bootstrap', f'gui/{os.getuid()}', str(path)],
                      'bootout_argv': ['/bin/launchctl', 'bootout', f'gui/{os.getuid()}/{job["Label"]}']}))


if __name__ == '__main__':
    main()

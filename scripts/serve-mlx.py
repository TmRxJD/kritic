#!/usr/bin/env python3
"""Start an explicitly selected upstream Kev MLX service, detached and loopback-only.
Requires Python 3.12+ and an upstream checkout with `uv sync --extra serve --no-dev`.
Does not install dependencies, choose another model, or fall back to CPU.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tomllib

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--config', required=True, type=Path)
p.add_argument('--kev-dir', required=True, type=Path)
p.add_argument('--log-dir', required=True, type=Path)
a = p.parse_args()
if sys.platform != 'darwin' or platform.machine() != 'arm64':
    p.error('native MLX serving requires Apple Silicon macOS')
cfg = tomllib.loads(a.config.read_text())
k = cfg['kev']
if cfg['review']['backend'] != 'local' or k['local_runtime'] != 'mlx' or k.get('cpu_only', False):
    p.error('select a local MLX profile with CPU fallback disabled')
from urllib.parse import urlsplit
url = urlsplit(k['native_url'])
if url.scheme != 'http' or url.hostname != '127.0.0.1' or url.path not in ('', '/') or url.query or url.fragment or url.username or url.password:
    p.error('launcher requires an HTTP 127.0.0.1 origin')
run = k['native_run']
if not run.startswith('jaredpalmer/kev-') or '@' not in run:
    p.error('pin native_run to an explicit upstream Hub revision before launching')
if k['native_dtype'] != 'bfloat16':
    p.error('this launcher supports the upstream bf16 MLX path only')
ram = int(subprocess.check_output(['sysctl', '-n', 'hw.memsize']))
# Upstream adapter merging briefly holds extra weights. Do not attempt 4B on an 8GB M1.
if 'kev-0.8b@' not in run and ram < 24 * 1024**3:
    p.error('larger checkpoint requires at least 24 GiB for this launcher; profile selection needs no weights')
kev = a.kev_dir.resolve()
python = kev / '.venv/bin/python'
if not python.is_file():
    p.error(f'{python} missing; sync the upstream serve environment first')
# Port occupancy is checked before any model download/load.
import socket
try:
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', url.port or 80))
except OSError as exc:
    p.error(f'loopback port unavailable; no service started: {exc}')
a.log_dir.mkdir(parents=True, exist_ok=True)
pidfile = a.log_dir / 'kev-mlx.pid'
if pidfile.exists():
    p.error(f'{pidfile} exists; inspect the recorded process before reusing this log directory')
env = dict(os.environ, KEV_BACKEND='mlx', KEV_DTYPE='bf16', KEV_MERGE='1', KEV_TRUNCATE_STATES='0',
           KEV_PREFIX_CACHE='1', KEV_PREFIX_MAX_TOKENS=str(k['max_seq_tokens']), PYTHONUNBUFFERED='1')
# Clear opt-in upstream transforms/temperature overrides for reproducible profile behavior.
for name in ('KEV_TEMPERATURE', 'KEV_LORA_SCALE', 'KEV_DATE_FACTS', 'KEV_API_KEY'):
    env.pop(name, None)
with (a.log_dir / 'kev-mlx.log').open('ab') as log:
    child = subprocess.Popen([str(python), '-m', 'kev.serve', '--run', run, '--device', 'mps',
                              '--host', '127.0.0.1', '--port', str(url.port or 80)],
                             cwd=kev, env=env, stdin=subprocess.DEVNULL, stdout=log,
                             stderr=subprocess.STDOUT, start_new_session=True)
pidfile.write_text(str(child.pid) + '\n')
print(json.dumps({'pid': child.pid, 'log': str(a.log_dir / 'kev-mlx.log'), 'endpoint': k['native_url'], 'run': run}))

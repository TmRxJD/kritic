"""Hermes block adapter for the canonical message contract."""
import json
import os
from pathlib import Path
import subprocess  # nosec B404
import sys

_HERE = Path(__file__).resolve().parent


def register(ctx):
    path = Path(json.loads((_HERE / 'contract.json').read_text(encoding='utf8'))['hook'])
    if not path.is_absolute() or not path.is_file():
        raise ValueError('Install the agent message hook and its absolute contract.json path')
    def before(tool_name, args, **kwargs):
        name = tool_name.rsplit('__', 1)[-1].rsplit('.', 1)[-1]
        if name not in ('message_agent', 'delegate_task'):
            return None
        try:
            result = subprocess.run(  # nosec B603
                [sys.executable, '-I', str(path), 'hermes'],
                input=json.dumps({'tool_name': tool_name, 'args': args}), capture_output=True,
                text=True, encoding='utf8', timeout=5, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
            if result.returncode or len(result.stdout.encode()) > 100_000:
                raise ValueError('Message hook failed or exceeded output limit')
            return json.loads(result.stdout) or None
        except (ValueError, OSError, subprocess.TimeoutExpired) as error:
            return {'action': 'block', 'message': f'Agent message contract unavailable: {error}'}

    ctx.register_hook('pre_tool_call', before)

"""Hermes authenticated turn-context adapter. No gateway rewriting or task execution."""
from collections import OrderedDict
import hashlib
import json
import logging
import os
from pathlib import Path
import subprocess  # nosec B404
import threading

_seen = OrderedDict()
_lock = threading.Lock()
_log = logging.getLogger('kritic.prompt')


def before_model(user_message='', session_id='', task_id='', turn_id='', **kwargs):
    if os.environ.get('KRITIC_PROMPT_INTERNAL') or not isinstance(user_message, str):
        return None
    if not user_message.strip() or len(user_message.encode('utf8')) > 65536:
        return None
    # Hermes' is_first_turn means first conversation turn, not first model call.
    # Do not use it to accidentally disable critique on every subsequent prompt.
    if not isinstance(task_id, str) or not all(isinstance(v, str) and v and len(v)<=512 for v in (session_id, turn_id)):
        _log.warning('Prompt review needs actual session_id and turn_id; original retained')
        return None
    key = (session_id, task_id, turn_id, hashlib.sha256(user_message.encode()).hexdigest())
    with _lock:
        if key in _seen:
            return None
        _seen[key] = True
        while len(_seen) > 128:
            _seen.popitem(last=False)
    try:
        executable = os.environ.get('KRITIC_PROMPT_EXE')
        if not executable:
            config = Path(__file__).resolve().parent / 'prompt.json'
            executable = json.loads(config.read_text(encoding='utf8'))['kritic']
        program = Path(executable)
        if not program.is_absolute() or not program.is_file():
            raise ValueError('Set KRITIC_PROMPT_EXE or prompt.json kritic to the matching absolute Kritic binary')
        result = subprocess.run(  # nosec B603
            [str(program), 'prompt', '--hook', 'hermes'],
            input=json.dumps({'prompt': user_message, 'session_id': session_id, 'turn_id': turn_id}),
            capture_output=True, text=True, encoding='utf8', timeout=10,
            creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
        if result.returncode or len(result.stdout.encode()) > 65536:
            raise ValueError('Prompt child failed or exceeded the output bound; original retained')
        report = json.loads(result.stdout)
        if report == {}:
            return None
        if not isinstance(report, dict) or set(report) != {'context'} or not isinstance(report['context'], str):
            raise ValueError('Prompt child returned an unsupported context contract')
        return {'context': report['context']}
    except (ValueError, TypeError, OSError, KeyError, subprocess.TimeoutExpired) as error:
        _log.warning('Prompt review unavailable: %s', error)
        return None


def register(ctx):
    if not callable(getattr(ctx, 'register_hook', None)):
        raise RuntimeError('This Hermes runtime does not support ctx.register_hook')
    ctx.register_hook('pre_llm_call', before_model)

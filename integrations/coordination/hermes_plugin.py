"""Hermes Python-plugin adapter; verifies real block-directive support at registration."""
import json
import os
from pathlib import Path
import subprocess  # nosec B404
import sys
import uuid

_HERE = Path(__file__).resolve().parent


def coordinator_path():
    config = _HERE / 'coordinator.json'
    configured = os.environ.get('KRITIC_COORDINATION_SCRIPT')
    if not configured and config.is_file():
        configured = json.loads(config.read_text(encoding='utf8'))['coordinator']
    script = Path(configured) if configured else _HERE / 'coordinator.py'
    if not script.is_absolute() or not script.is_file():
        raise ValueError('Install coordinator.py and configure its absolute path in coordinator.json')
    return script


def invoke(event, payload):
    # sys.executable and the configured integration path are trusted host setup.
    child = subprocess.run(  # nosec B603
        [sys.executable, '-I', str(coordinator_path()), 'hook', 'hermes', event],
        input=json.dumps(payload), capture_output=True, text=True, encoding='utf8', timeout=15,
        creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    if child.returncode:
        raise ValueError((child.stderr or child.stdout)[-1000:])
    if len(child.stdout.encode()) > 100_000:
        raise ValueError('Coordinator hook response is oversized')
    return json.loads(child.stdout)


def before_tool(tool_name='', args=None, task_id='', session_id='', **kwargs):
    try:
        from tools.file_tools_paths import _resolve_base_dir, _terminal_env_type_for_task, _resolve_path_for_task
        if _terminal_env_type_for_task(task_id) != 'local':
            return {'action': 'block', 'message': 'Coordination needs a coordinator in the actual remote/container namespace; host path ownership cannot be assumed.'}
        root = str(_resolve_base_dir(task_id, container_paths=False))
        payload = {'session_id': session_id or task_id, 'agent_id': task_id or 'main',
                   'cwd': root, 'tool_name': tool_name, 'tool_input': dict(args or {})}
        if tool_name == 'write_file':
            path = payload['tool_input'].get('path')
            if not isinstance(path, str):
                raise ValueError('write_file requires a path')
            payload['tool_input'] = {'path': str(_resolve_path_for_task(path, task_id))}
        elif tool_name == 'patch':
            # Reuse Hermes' actual structured patch parser and task path resolver.
            mode = payload['tool_input'].get('mode', 'replace')
            paths = []
            if mode == 'replace':
                path = payload['tool_input'].get('path')
                if not isinstance(path, str) or not path:
                    raise ValueError('patch replace mode requires a path')
                paths.append(str(_resolve_path_for_task(path, task_id)))
            elif mode == 'patch':
                from tools.patch_parser import parse_v4a_patch
                patch_text = payload['tool_input'].get('patch')
                if not isinstance(patch_text, str) or len(patch_text.encode()) > 1_000_000:
                    raise ValueError('invalid or oversized patch')
                operations, error = parse_v4a_patch(patch_text)
                if error or not operations:
                    raise ValueError(error or 'patch has no operations')
                for operation in operations:
                    paths.append(str(_resolve_path_for_task(operation.file_path, task_id)))
                    if operation.new_path:
                        paths.append(str(_resolve_path_for_task(operation.new_path, task_id)))
            else:
                raise ValueError('unsupported patch mode')
            if len(paths) > 128:
                raise ValueError('Too many patch targets')
            payload['tool_input'] = {'patch': '\n'.join('*** Update File: ' + path for path in paths)}
        return invoke('pretool', payload) or None
    except Exception as error:
        return {'action': 'block', 'message': f'Worktree coordination unavailable: {error}. Repair setup before edits.'}


def session_end(session_id='', **kwargs):
    if session_id:
        invoke('end', {'session_id': session_id})


def register(ctx):
    from hermes_cli.plugins import get_pre_tool_call_directive
    marker = 'coordination-capability-' + uuid.uuid4().hex

    def probe(tool_name='', **kwargs):
        return {'action': 'block', 'message': marker} if tool_name == marker else None

    ctx.register_hook('pre_tool_call', probe)
    action, message = get_pre_tool_call_directive(marker, {}, session_id=marker)
    if action != 'block' or message != marker:
        raise RuntimeError('This Hermes dispatcher did not honor the coordination blocking probe; enforcement is unavailable')
    ctx.register_hook('pre_tool_call', before_tool)
    ctx.register_hook('on_session_end', session_end)

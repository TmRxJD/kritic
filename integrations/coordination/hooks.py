"""Harness wire adapters. Scope checks protect supported structured writes.

Opaque shell/MCP effects remain cooperative; they are not a filesystem sandbox.
"""
import json
import os
from pathlib import Path
import re
import time

from paths import git, identity, local_path, repository
from registry import actor_id, actors, audit, authorized, observe, transaction

READ_TOOLS = {'Read', 'Glob', 'Grep', 'read_file', 'search_files', 'list_directory', 'WebSearch', 'WebFetch'}
STRUCTURED_TOOLS = {'Write', 'Edit', 'MultiEdit', 'write_file', 'edit_file', 'apply_patch', 'patch', 'apply_diff'}
LIFECYCLE = {'session': 'SessionStart', 'prompt': 'UserPromptSubmit', 'subagent': 'SubagentStart'}


def response(host, message, deny=False, event='pretool'):
    if host == 'hermes':
        return {'action': 'block', 'message': message} if deny else {'context': message}
    name = LIFECYCLE.get(event, 'PreToolUse')
    fields = {'hookEventName': name}
    if deny:
        fields.update(permissionDecision='deny', permissionDecisionReason=message)
    else:
        fields['additionalContext'] = message
    return {'hookSpecificOutput': fields}


def targets(tool, data):
    if not isinstance(data, dict):
        return None
    if tool == 'apply_patch' or tool in ('patch', 'apply_diff'):
        patch = data.get('command') or data.get('patch') or data.get('input')
        if not isinstance(patch, str):
            return None
        paths = re.findall(r'^\*\*\* (?:Add File|Update File|Delete File|Move to): (.+)$', patch, flags=re.M)
        return paths or None
    if tool in ('Write', 'Edit', 'MultiEdit', 'write_file', 'edit_file'):
        path = data.get('file_path') or data.get('path')
        return [path] if isinstance(path, str) else None
    return None


def target_roots(paths, execution_cwd):
    """Inspect every target; a non-repository first target cannot bypass later ones."""
    original = git(execution_cwd, 'rev-parse', '--show-toplevel', allow_failure=True)
    original = Path(os.path.abspath(original.strip())) if original else None
    cache = {}
    roots = []
    deadline = time.monotonic() + 10
    for path in paths:
        target = Path(os.path.abspath(Path(execution_cwd) / path))
        if original and (not Path(path).is_absolute() or target.is_relative_to(original)):
            roots.append(str(original))
            continue
        current = target.parent
        visited = []
        found = None
        while True:
            if time.monotonic() > deadline:
                raise ValueError('Target discovery exceeded its budget; split the patch')
            key = str(current)
            if key in cache:
                found = cache[key]
                break
            visited.append(key)
            if current.is_dir():
                root = git(current, 'rev-parse', '--show-toplevel', allow_failure=True)
                if root:
                    found = root.strip()
                    break
            if current == current.parent:
                break
            current = current.parent
        for key in visited:
            cache[key] = found
        roots.append(found)
    return roots


def hook(host, event, payload):
    if host not in ('codex', 'claude', 'hermes'):
        raise ValueError('Unsupported harness')
    session = payload.get('session_id') or payload.get('sessionId') or payload.get('task_id')
    agent = payload.get('agent_id') or 'main'
    key = actor_id(host, session, agent)
    if event in ('end', 'subagent_end'):
        if event == 'subagent_end' and agent == 'main':
            raise ValueError('Subagent end event requires its actual agent_id')
        with transaction() as db:
            audit(db, key, 'session_ended', {'event': event})
            if event == 'end' and agent == 'main':
                db.execute('UPDATE actors SET ended=1,generation=generation+1 WHERE host=? AND session=?', (host, session))
            else:
                db.execute('UPDATE actors SET ended=1,generation=generation+1 WHERE id=?', (key,))
        return {}
    data = payload.get('tool_input') or payload.get('args') or {}
    tool = payload.get('tool_name', '')
    if event == 'pretool' and tool.endswith('worktree_evaluate'):
        if data.get('action') in ('declare', 'release'):
            declaration = data.get('request') or {}
            if declaration.get('host') != host or declaration.get('session_id') != session:
                return response(host, f'Use actual hook identity host={host}, session_id={session}; do not alter another session claim.', True)
            if payload.get('agent_id') and declaration.get('agent_id', 'main') != agent:
                return response(host, 'Use the actual agent_id from the hook context.', True)
        return {}
    cwd = data.get('cwd') or data.get('workdir') or payload.get('cwd') or os.getcwd()
    original_cwd = cwd
    local_path(cwd)
    changed = targets(tool, data)
    roots = []
    if changed:
        if len(changed) > 128:
            raise ValueError('Too many structured write targets; split the operation')
        for path in changed:
            local_path(path)
        roots = target_roots(changed, original_cwd)
        candidates = [root for root in roots if root is not None]
        if not candidates:
            return {}
        cwd = candidates[0]
    elif git(cwd, 'rev-parse', '--show-toplevel', allow_failure=True) is None:
        return {}
    info = repository(cwd)
    with transaction() as db:
        if event == 'pretool' and not payload.get('agent_id'):
            owners = [row for row in actors(db, info['repo']) if row['host'] == host and row['session'] == session
                      and row['declaration'] and row['declaration'].get('access') != 'read_only' and identity(row['worktree']) == identity(info['worktree'])]
            if len(owners) == 1:
                agent = owners[0]['agent']
                key = owners[0]['id']
            else:
                previous = db.execute('SELECT * FROM actors WHERE id=?', (key,)).fetchone()
                if previous and not previous['ended'] and (previous['repo'] != info['repo'] or (previous['declaration'] and identity(previous['worktree']) != identity(info['worktree']))):
                    return response(host, 'This worktree has no unambiguous declaration for the session. Declare its actual actor and scope before editing; the existing reservation is preserved.', True)
        observe(db, host, session, agent, info)
        active = actors(db, info['repo'])
        if event in LIFECYCLE:
            message = (f'Worktree coordination actor {key}. {len(active)} registered agent(s) in this repository. '
                       'When multiple agents are present, each must declare a separate worktree, task, change_summary, '
                       'and file/folder scopes with purposes through worktree_evaluate action=declare before editing. '
                       'Use action=schema for exact fields and action=status to inspect reservations. '
                       'Directory scopes end in /. Narrow or reassign overlapping scopes. No automatic merge or cleanup.')
            return response(host, message, event=event)
        if tool in READ_TOOLS:
            return {}
        if tool.endswith('cli_run') and isinstance(data, dict):
            args = data.get('args')
            program = Path(str(data.get('program', ''))).name.casefold()
            if program in ('python', 'python.exe', 'python3') and isinstance(args, list) and len(args) == 2 and args[0] == '-I':
                coordinator = Path(__file__).resolve().parent / 'coordinator.py'
                if identity(args[1]) == identity(coordinator):
                    bootstrap = json.loads(data.get('stdin') or '{}')
                    if bootstrap.get('action') in ('schema', 'status', 'declare', 'release'):
                        if bootstrap.get('action') in ('declare', 'release'):
                            owner = bootstrap.get('request') or {}
                            if owner.get('host') != host or owner.get('session_id') != session:
                                return response(host, 'Use your actual host/session identity for coordination bootstrap.', True)
                        return {}
            if program in ('git', 'git.exe') and isinstance(args, list) and args[:2] == ['worktree', 'add'] and all(isinstance(arg, str) for arg in args) and not any(arg.startswith('--force') or (arg.startswith('-') and not arg.startswith('--') and ('f' in arg or 'B' in arg)) for arg in args):
                return {}
        if tool.endswith(('create_worktree', 'attach_worktree')):
            return {}
        current = next(row for row in active if row['id'] == key)
        if len(active) <= 1 and not current['declaration']:
            return {}
        if changed and (None in roots or len({identity(root) for root in roots if root}) != 1):
            return response(host, 'Split structured writes spanning repository/non-repository paths or multiple worktrees, then use the correct declared owner for each.', True)
        undeclared = [row['id'] for row in active if not row['declaration']]
        if undeclared:
            return response(host, f'Declare worktree, task, change_summary and owned scopes before edits. Missing declarations: {undeclared}. Use worktree_evaluate action=declare. Read tools and coordination APIs remain available.', True)
        locations = [identity(row['worktree']) for row in active if row['declaration'].get('access') != 'read_only']
        if len(set(locations)) != len(locations):
            return response(host, 'Registered agents share a worktree. Declare distinct worktrees before edits; preserve all existing changes.', True)
        effective = Path(original_cwd).resolve()
        if changed:
            if payload.get('agent_id') and current['declaration'].get('access') == 'read_only':
                return response(host, 'This agent declared read_only access. Declare an editing task and nonoverlapping write scope before modifying files.', True)
            absolute = [str((effective / p).resolve()) if not Path(p).is_absolute() else str(Path(p).resolve()) for p in changed]
            owners = [row for row in active if row['host'] == host and row['session'] == session and row['declaration'].get('access') != 'read_only' and authorized(row['declaration'], absolute)]
            if len(owners) != 1:
                return response(host, f'Write targets are outside one declared owner scope or escape its worktree: {changed}. Expand the actual task scope without overlapping peers, or use the correct worktree.', True)
            if agent != 'main' and owners[0]['agent'] != agent:
                return response(host, 'Tool agent identity does not match the declared worktree owner.', True)
            return {}
        if tool in STRUCTURED_TOOLS:
            return response(host, 'Unsupported structured write format. Use an explicit file path or supported apply_patch headers so scope can be checked.', True)
        # Opaque commands cannot be reliably converted to filesystem effects.
        owners = [row for row in active if row['host'] == host and row['session'] == session and identity(row['worktree']) == identity(effective)]
        if payload.get('agent_id'):
            owners = [row for row in owners if row['id'] == key]
        else:
            editors = [row for row in owners if row['declaration'].get('access') != 'read_only']
            owners = editors or owners
        if len(owners) != 1:
            return response(host, 'Run this operation with an explicit cwd equal to its declared worktree. Opaque command effects cannot be inferred safely from the session cwd.', True)
        return {}

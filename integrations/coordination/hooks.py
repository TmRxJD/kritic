"""Harness wire adapters. Scope checks protect supported structured writes.

Opaque shell/MCP effects remain cooperative; they are not a filesystem sandbox.
"""
import json
import os
from pathlib import Path
import re
import shlex
import time

from paths import git, identity, local_path, repository
from registry import actor_id, actors, audit, authorized, observe, pending_transfer, transfer_fence, transaction

READ_TOOLS = {'Read', 'Glob', 'Grep', 'read_file', 'search_files', 'list_directory', 'WebSearch', 'WebFetch'}
STRUCTURED_TOOLS = {'Write', 'Edit', 'MultiEdit', 'write_file', 'edit_file', 'apply_patch', 'patch', 'apply_diff'}
LIFECYCLE = {'session': 'SessionStart', 'prompt': 'UserPromptSubmit', 'subagent': 'SubagentStart'}
SETUP_TOOLS = {'search_tools', 'search_tool', 'tool_search', 'discover_tools', 'list_tools',
               'send_message', 'send_input', 'send_message_to_thread', 'read_thread', 'list_threads',
               'list_agents', 'wait_agent', 'get_worktree_creation_status'}


def setup_help(host, session, agent, cwd):
    """Give a usable declaration route even when the MCP tool is not loaded."""
    coordinator = (Path(__file__).resolve().parent / 'coordinator.py').as_posix()
    value = {'action': 'declare', 'request': {'host': host, 'session_id': session,
             'agent_id': agent, 'worktree': str(cwd), 'task': 'REPLACE with your actual task',
             'change_summary': 'REPLACE with your intended changes',
             'scopes': [{'path': 'REPLACE with an exact file or folder/', 'purpose': 'REPLACE with its purpose'}]}}
    command = f'python -I "{coordinator}" --request-json ' + shlex.quote(json.dumps(value))
    return (f'Your actual identity: host={host}, session_id={session}, agent_id={agent}. '
            'Discover worktree_evaluate and call action=schema, then action=declare with your own identity. '
            'If unavailable, use the native command tool (exec_command or Bash) with this exact installed '
            'coordinator command, replacing only task, change_summary, worktree and scopes with actual values: '
            f'{command}. This restricted bootstrap is allowed before declaration; arbitrary shell commands are not. '
            'Tool discovery and agent messaging remain available. Ask undeclared peers to declare themselves; '
            'never invent their tasks or impersonate them. If the user authorized continuing an unavailable owner\'s work, '
            'discover recover via action=schema and inspect one exact source assignment before accepting it; no old-owner release is required. '
            'Reconnect MCP to load the newly installed tool.')


def bootstrap(tool, data, host, session, agent):
    """Accept only an exact coordinator invocation, not general shell execution."""
    name = tool.rsplit('__', 1)[-1].rsplit('.', 1)[-1]
    args = None
    if name == 'cli_run':
        program, args = data.get('program', ''), data.get('args')
    elif name in ('exec_command', 'Bash', 'run_command'):
        command = data.get('cmd') or data.get('command')
        if not isinstance(command, str):
            return False
        try:
            tokens = shlex.split(command)
        except ValueError:
            return False
        if not tokens:
            return False
        program, args = tokens[0], tokens[1:]
    else:
        return False
    if Path(str(program)).name.casefold() not in ('python', 'python.exe', 'python3') or not isinstance(args, list):
        return False
    coordinator = Path(__file__).resolve().parent / 'coordinator.py'
    if len(args) not in (2, 4) or args[0] != '-I' or identity(args[1]) != identity(coordinator):
        return False
    if len(args) == 4:
        if args[2] != '--request-json':
            return False
        raw = args[3]
    else:
        if name != 'cli_run':
            return False
        raw = data.get('stdin') or '{}'
    value = json.loads(raw)
    if value.get('action') not in ('schema', 'status', 'declare', 'release', 'transfer', 'recover'):
        return False
    if value.get('action') in ('declare', 'release', 'transfer', 'recover'):
        owner = value.get('request') or {}
        if owner.get('host') != host or owner.get('session_id') != session or (agent is not None and owner.get('agent_id', 'main') != agent):
            raise ValueError('Use your actual host/session/agent identity for coordination bootstrap')
    return True


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
                for row in db.execute('SELECT id FROM actors WHERE host=? AND session=?',(host,session)).fetchall():
                    db.execute('UPDATE actors SET ended=1 WHERE id=?',(row['id'],))
            else:
                db.execute('UPDATE actors SET ended=1 WHERE id=?', (key,))
        return {}
    data = payload.get('tool_input') or payload.get('args') or {}
    tool = payload.get('tool_name', '')
    if event == 'pretool':
        name = tool.rsplit('__', 1)[-1].rsplit('.', 1)[-1]
        if name in SETUP_TOOLS or tool in ('functions.search', 'mcp__tool_search__search') or (tool.startswith('tool_search.') and name in ('search', 'msearch')):
            return {}
        if bootstrap(tool, data, host, session, payload.get('agent_id')):
            return {}
    if event == 'pretool' and tool.endswith('worktree_evaluate'):
        if data.get('action') in ('declare', 'release', 'transfer', 'recover'):
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
        db.execute('INSERT OR IGNORE INTO actor_epochs VALUES (?,0,NULL,NULL)',(key,))
        if event == 'prompt':
            db.execute('UPDATE actor_epochs SET epoch=epoch+1,boundary_epoch=NULL,boundary_generation=NULL WHERE actor=?',(key,))
        if event == 'boundary':
            generation = db.execute('SELECT generation FROM actors WHERE id=?',(key,)).fetchone()[0]
            db.execute('UPDATE actor_epochs SET boundary_epoch=epoch,boundary_generation=? WHERE actor=?',(generation,key))
            return {}
        active = actors(db, info['repo'])
        if event in LIFECYCLE:
            message = (f'Worktree coordination actor {key}. {len(active)} registered agent(s) in this repository. '
                       'When multiple agents are present, each must declare a separate worktree, task, change_summary, '
                       'and file/folder scopes with purposes through worktree_evaluate action=declare before editing. '
                       'Use action=schema for exact fields and action=status to inspect reservations. '
                       'Directory scopes end in /. Narrow or reassign overlapping scopes. No automatic merge or cleanup. '
                       + setup_help(host, session, agent, info['worktree']))
            return response(host, message, event=event)
        if tool in READ_TOOLS:
            return {}
        if pending_transfer(db,key):
            return response(host,'This source is quiescent for ownership transfer. Read/recovery/messaging and coordination APIs remain available. Cancel the pending transfer before resuming edits; never overwrite successor work.',True)
        if event == 'pretool' and not tool.endswith(('session_critic','handoff_critic')):
            metadata = (Path(info['worktree'])/'.kritic/session-state').resolve()
            only_metadata = changed and all((Path(original_cwd)/path).resolve().is_relative_to(metadata) for path in changed)
            if not only_metadata:
                db.execute('UPDATE actor_epochs SET boundary_epoch=NULL,boundary_generation=NULL WHERE actor=?',(key,))
        if tool.endswith('cli_run') and isinstance(data, dict):
            args = data.get('args')
            program = Path(str(data.get('program', ''))).name.casefold()
            if program in ('git', 'git.exe') and isinstance(args, list) and args[:2] == ['worktree', 'add'] and all(isinstance(arg, str) for arg in args) and not any(arg.startswith('--force') or (arg.startswith('-') and not arg.startswith('--') and ('f' in arg or 'B' in arg)) for arg in args):
                return {}
        if tool.endswith(('create_worktree', 'attach_worktree')):
            return {}
        current = next(row for row in active if row['id'] == key)
        if len(active) <= 1 and not current['declaration']:
            return {}
        if changed and (None in roots or len({identity(root) for root in roots if root}) != 1):
            return response(host, 'Split structured writes spanning repository/non-repository paths or multiple worktrees, then use the correct declared owner for each.', True)
        if not current['declaration']:
            undeclared = [key]
            return response(host, f'Declare worktree, task, change_summary and owned scopes before edits. Missing declarations: {undeclared}. ' + setup_help(host, session, agent, info['worktree']), True)
        locations = [identity(row['worktree']) for row in active if row['declaration'] and row['declaration'].get('access') != 'read_only']
        if len(set(locations)) != len(locations):
            return response(host, 'Registered agents share a worktree. Declare distinct worktrees before edits; preserve all existing changes.', True)
        effective = Path(original_cwd).resolve()
        if changed:
            if payload.get('agent_id') and current['declaration'].get('access') == 'read_only':
                return response(host, 'This agent declared read_only access. Declare an editing task and nonoverlapping write scope before modifying files.', True)
            absolute = [str((effective / p).resolve()) if not Path(p).is_absolute() else str(Path(p).resolve()) for p in changed]
            owners = [row for row in active if row['host'] == host and row['session'] == session and row['declaration'] and row['declaration'].get('access') != 'read_only' and authorized(row['declaration'], absolute)]
            if len(owners) != 1:
                return response(host, f'Write targets are outside one declared owner scope or escape its worktree: {changed}. Expand the actual task scope without overlapping peers, or use the correct worktree.', True)
            if agent != 'main' and owners[0]['agent'] != agent:
                return response(host, 'Tool agent identity does not match the declared worktree owner.', True)
            return {}
        if tool in STRUCTURED_TOOLS:
            return response(host, 'Unsupported structured write format. Use an explicit file path or supported apply_patch headers so scope can be checked.', True)
        # Opaque commands cannot be reliably converted to filesystem effects.
        owners = [row for row in active if row['host'] == host and row['session'] == session and row['declaration'] and identity(row['worktree']) == identity(effective)]
        if payload.get('agent_id'):
            owners = [row for row in owners if row['id'] == key]
        else:
            editors = [row for row in owners if row['declaration'].get('access') != 'read_only']
            owners = editors or owners
        if len(owners) != 1:
            return response(host, 'Run this operation with an explicit cwd equal to its declared worktree. Opaque command effects cannot be inferred safely from the session cwd.', True)
        if transfer_fence(db,key,info):
            return response(host,'This source transferred its writer role and cannot run opaque operations in the successor worktree. Use supported reads or a separate declared assignment.',True)
        return {}

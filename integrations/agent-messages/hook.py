"""PreToolUse validator; invalid messages are rejected, never silently shortened."""
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from contract import validate


def messages(tool, args):
    name = tool.rsplit('__', 1)[-1].rsplit('.', 1)[-1]
    if name in ('spawn_agent', 'send_message', 'followup_task', 'message_agent'):
        return [('message', args.get('message'), name == 'spawn_agent')]
    if name in ('Task', 'Agent', 'create_thread', 'send_message_to_thread'):
        return [('prompt', args.get('prompt'), name in ('Task', 'Agent', 'create_thread'))]
    if name == 'SendMessage' and args.get('type', 'message') in ('message', 'broadcast'):
        return [('content', args.get('content'), False)]
    if name == 'delegate_task':
        if args.get('action') == 'steer':
            return [('message', args.get('message'), False)]
        if args.get('action') in ('list', 'stop'):
            return []
        tasks = args.get('tasks', [args])
        if not isinstance(tasks, list) or not tasks or len(tasks) > 16 or any(not isinstance(task, dict) for task in tasks):
            return [('tasks', None, True)]
        result = []
        for i, task in enumerate(tasks):
            if task.get('context'):
                # Keep one canonical message instead of an unchecked second prose channel.
                return [(f'tasks[{i}].context: put references inside the task JSON context array', None, True)]
            result.append((f'tasks[{i}].goal', task.get('goal'), True))
        return result
    return []


def violation(tool, args):
    if not isinstance(args, dict):
        return None
    for field, raw, task_only in messages(tool, args):
        try:
            validate(raw, task_only)
        except (ValueError, TypeError, RecursionError) as error:
            return (f'Agent message contract v1: {field}: {error}. '
                    'Task: {"kind":"task","goal":"...","scope":["..."],"done_when":["..."]}. '
                    'Result: {"kind":"result","summary":"...","changes":[],"checks":[{"name":"...","status":"not_run","detail":"reason"}],"evidence":["path:line or command output"]}. '
                    f'Full schema: python -I "{(Path(__file__).resolve().parent / "contract.py").as_posix()}". '
                    'Preserve exact paths, IDs, failures, uncertainty and required constraints. No pleasantries or raw log dumps. '
                    'This validates structure and size, not truth, completeness, ownership or authorization.')
    return None


def response(host, reason):
    if not reason:
        return {}
    if host == 'hermes':
        return {'action': 'block', 'message': reason}
    return {'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny', 'permissionDecisionReason': reason}}


def main():
    host = sys.argv[1] if len(sys.argv) == 2 else None
    if host not in ('codex', 'claude', 'hermes'):
        raise ValueError('Expected host codex, claude or hermes')
    try:
        raw = sys.stdin.buffer.read(1_000_001)
        if len(raw) > 1_000_000:
            raise ValueError('Hook input exceeds 1 MB')
        event = json.loads(raw)
        reason = violation(event.get('tool_name', ''), event.get('tool_input', event.get('args', {})))
    except (ValueError, TypeError, AttributeError, RecursionError) as error:
        reason = f'Agent message contract hook error: {error}'
    print(json.dumps(response(host, reason)))


if __name__ == '__main__':
    main()

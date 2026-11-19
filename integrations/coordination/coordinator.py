"""Shared coordinator CLI and JSON hook entry point. All Git inspections are read-only."""
import json
from pathlib import Path
import sys

# -I intentionally excludes cwd; import only this installed integration's siblings.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from evaluate import evaluate, inspect
from hooks import hook, response
from registry import declare, fields, release, status
from transfers import schema as transfer_schema, transfer


def schema():
    text = {'type': 'string', 'minLength': 1, 'maxLength': 2000}
    inspect_schema = {'type': 'object', 'additionalProperties': False, 'required': ['worktree', 'main_ref'],
        'properties': {'worktree': text, 'main_ref': dict(text, pattern='^refs/', description='Refresh remote refs separately before inspecting'),
            'worktrees': {'type': 'array', 'minItems': 1, 'maxItems': 32, 'uniqueItems': True, 'items': text},
            'owner_actor_ids': {'type': 'object', 'maxProperties': 32, 'additionalProperties': text}}}
    inspect_schema['properties']['paths'] = {'type': 'array', 'maxItems': 64, 'items': text, 'description': 'Optional exact repository-relative files or directory prefixes to bound this inspection'}
    review = {'type': 'object', 'additionalProperties': False,
        'required': ['worktree', 'head', 'owner_actor_id', 'owner_generation', 'hunk_id', 'decision', 'reason'],
        'properties': {'worktree': text, 'head': dict(text, pattern='^[0-9a-f]{40}([0-9a-f]{24})?$'),
            'owner_actor_id': text, 'owner_generation': {'type': 'integer', 'minimum': 1}, 'hunk_id': dict(text, pattern='^[0-9a-f]{64}$'),
            'decision': {'enum': ['include', 'exclude', 'defer']}, 'reason': text}}
    return {
        'declaration': {'type': 'object', 'additionalProperties': False,
            'required': ['host', 'session_id', 'worktree', 'task', 'change_summary', 'scopes'],
            'properties': {'host': {'enum': ['codex', 'claude', 'hermes']}, 'session_id': text, 'agent_id': text,
                'worktree': text, 'task': text, 'change_summary': text, 'access': {'enum': ['edit', 'read_only'], 'default': 'edit'},
                'scopes': {'type': 'array', 'minItems': 1, 'maxItems': 64, 'items': {'type': 'object', 'additionalProperties': False,
                    'required': ['path', 'purpose'], 'properties': {'path': text, 'purpose': text}}}}},
        'inspect': inspect_schema,
        'evaluate': {'type': 'object', 'additionalProperties': False, 'required': ['snapshot_request', 'main_revision', 'reviews'],
            'properties': {'snapshot_request': inspect_schema, 'main_revision': dict(text, pattern='^[0-9a-f]{40}([0-9a-f]{24})?$'),
                'reviews': {'type': 'array', 'maxItems': 4096, 'items': review}}},
        'status': {'type': 'object', 'additionalProperties': False, 'required': ['worktree'],
            'properties': {'worktree': text, 'include_history': {'type': 'boolean'}}},
        'release': {'type': 'object', 'additionalProperties': False, 'required': ['host', 'session_id', 'reason'],
            'properties': {'host': {'enum': ['codex', 'claude', 'hermes']}, 'session_id': text, 'reason': text,
                'agent_id': text, 'expected_generation': {'type': 'integer', 'minimum': 1}}},
        'transfer': transfer_schema(),
        'semantics': 'File scopes are exact; directories end in /. Separate worktrees and nonoverlapping scopes are required for concurrent agents. All changes need grounded review. Inspect/evaluate perform no merge, reset, checkout, cleanup or remote fetch.',
        'coverage': 'Structured writes are scope checked; opaque shell/MCP effects are cooperative. Codex subagents can share session_id without agent_id on tool events; a unique declared worktree can identify the editing owner, not prove the caller identity.'}


def request(value):
    fields(value, ('action',), ('request',))
    action = value['action']
    data = value.get('request', {})
    if action == 'schema':
        if data:
            raise ValueError('schema action accepts no request fields')
        return schema()
    functions = {'declare': declare, 'status': status, 'release': release, 'inspect': inspect, 'evaluate': evaluate, 'transfer':transfer}
    if action not in functions:
        raise ValueError('Unknown coordination action')
    return functions[action](data)


def main():
    is_hook = len(sys.argv) == 4 and sys.argv[1] == 'hook'
    try:
        if len(sys.argv) == 3 and sys.argv[1] == '--request-json':
            raw = sys.argv[2].encode('utf8')
        elif len(sys.argv) == 1 or is_hook:
            raw = sys.stdin.buffer.read(1_000_001)
        else:
            raise ValueError('Use JSON stdin or --request-json <JSON>; hook mode is internal')
        if len(raw) > 1_000_000:
            raise ValueError('Request exceeds 1 MB')
        value = json.loads(raw)
        result = hook(sys.argv[2], sys.argv[3], value) if is_hook else request(value)
        output = json.dumps(result)
        if len(output.encode()) > 1_000_000:
            raise ValueError('Report exceeds 1 MB; narrow the selected worktrees')
        print(output)
    except Exception as error:
        if is_hook:
            print(json.dumps(response(sys.argv[2], f'Coordination error: {error}. Inspect status and repair the declaration before edits.', True)))
        else:
            print(json.dumps({'error': str(error)}))
            return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())

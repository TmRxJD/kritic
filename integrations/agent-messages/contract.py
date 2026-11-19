"""Bounded, model-free contract for agent-to-agent messages. Never rewrites facts."""
import json
import re

TEXT = {'type': 'string', 'minLength': 1, 'maxLength': 300}
REF = {'type': 'string', 'minLength': 1, 'maxLength': 600}


def items(value=TEXT, minimum=0, maximum=8):
    return {'type': 'array', 'minItems': minimum, 'maxItems': maximum, 'items': value}


CHECK = {'type': 'object', 'additionalProperties': False, 'required': ['name', 'status'],
         'properties': {'name': TEXT, 'status': {'enum': ['pass', 'fail', 'not_run']}, 'detail': TEXT},
         'allOf': [{'if': {'properties': {'status': {'enum': ['fail', 'not_run']}}}, 'then': {'required': ['detail']}}]}
KINDS = {
    'task': (['goal', 'scope', 'done_when'], {'goal': TEXT, 'scope': items(minimum=1), 'done_when': items(minimum=1),
             'worktree': REF, 'constraints': items(), 'context': items(REF)}),
    'result': (['summary', 'changes', 'checks', 'evidence'], {'summary': TEXT, 'changes': items(),
               'checks': items(CHECK, 1), 'evidence': items(REF, 1), 'remaining': items()}),
    'blocker': (['problem', 'evidence', 'needs'], {'problem': TEXT, 'evidence': items(REF, 1),
                'needs': items(minimum=1), 'attempted': items()}),
    'question': (['question'], {'question': TEXT, 'options': items(maximum=4), 'context': items(REF)}),
    'update': (['summary'], {'summary': TEXT, 'next': TEXT, 'refs': items(REF)}),
}
WORD_LIMITS = {'task': 240, 'result': 180, 'blocker': 120, 'question': 80, 'update': 80}
MAX_BYTES = 4096
PROSE = {'goal', 'summary', 'problem', 'question', 'next'}
POLITENESS = re.compile(r'^(?:(?:hi|hello|hey|dear)\b[ ,!:.-]*|please\b|(?:thanks|thank you)\b)|(?:thanks|thank you)[.! ]*$', re.I)


def schema():
    variants = []
    for kind, (required, properties) in KINDS.items():
        variants.append({'type': 'object', 'additionalProperties': False,
                         'required': ['kind', *required],
                         'properties': {'kind': {'enum': [kind]}, **properties}})
    return {'$schema': 'https://json-schema.org/draft/2020-12/schema', 'title': 'Agent message v1',
            'oneOf': variants, 'x-max-utf8-bytes': MAX_BYTES, 'x-word-limits': WORD_LIMITS}


def check(value, spec, path='$'):
    if 'enum' in spec and value not in spec['enum']:
        raise ValueError(f'{path}: expected one of {spec["enum"]}')
    kind = spec.get('type')
    if kind == 'object':
        if not isinstance(value, dict):
            raise ValueError(f'{path}: expected object')
        missing = set(spec['required']) - value.keys()
        unknown = value.keys() - spec['properties'].keys()
        if missing or unknown:
            raise ValueError(f'{path}: missing={sorted(missing)}, unknown={sorted(unknown)}')
        for key, child in value.items():
            check(child, spec['properties'][key], f'{path}.{key}')
    elif kind == 'array':
        if not isinstance(value, list) or not spec['minItems'] <= len(value) <= spec['maxItems']:
            raise ValueError(f'{path}: expected {spec["minItems"]}..{spec["maxItems"]} items')
        for i, child in enumerate(value):
            check(child, spec['items'], f'{path}[{i}]')
    elif kind == 'string':
        if not isinstance(value, str) or not value.strip() or len(value) > spec['maxLength']:
            raise ValueError(f'{path}: expected nonempty string, at most {spec["maxLength"]} characters')


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for child in value:
            yield from strings(child)
    elif isinstance(value, dict):
        for child in value.values():
            yield from strings(child)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'Duplicate message field: {key}')
        result[key] = value
    return result


def validate(raw, task_only=False):
    if not isinstance(raw, str) or len(raw.encode('utf8')) > MAX_BYTES:
        raise ValueError(f'Message must be a JSON object string of at most {MAX_BYTES} UTF-8 bytes')
    try:
        value = json.loads(raw, object_pairs_hook=unique_object)
    except (ValueError, RecursionError) as error:
        raise ValueError('Use one JSON message object without Markdown fences or introductory prose') from error
    kind = value.get('kind') if isinstance(value, dict) else None
    if not isinstance(kind, str) or kind not in KINDS:
        raise ValueError('kind must be task, result, blocker, question or update')
    if task_only and kind != 'task':
        raise ValueError('New agent assignments require kind=task with goal, scope and done_when')
    spec = next(variant for variant in schema()['oneOf'] if variant['properties']['kind']['enum'] == [kind])
    check(value, spec)
    if any(text.strip().casefold() in ('...', '…', 'todo', 'tbd', 'n/a', '<...>') for text in strings(value)):
        raise ValueError('Replace placeholders with actual scope, evidence, completion criteria or a specific blocker')
    words = sum(len(text.split()) for text in strings(value))
    if words > WORD_LIMITS[kind]:
        raise ValueError(f'{kind} exceeds {WORD_LIMITS[kind]} words; move long evidence to a permanent artifact and cite its path')
    for key in PROSE & value.keys():
        if POLITENESS.search(value[key]):
            raise ValueError(f'{key}: omit greetings, please, thanks and sign-offs; state the request or fact directly')
    for entry in value.get('checks', []):
        if entry['status'] in ('not_run', 'fail') and not entry.get('detail', '').strip():
            raise ValueError('Failed or unrun checks require detail with the failure or reason')
    return value


if __name__ == '__main__':
    print(json.dumps(schema(), indent=2))

"""Persistent transactional ownership. Release changes metadata only; work is never removed."""
import contextlib
import json
import os
from pathlib import Path
import sqlite3
import time
import re

from paths import contains, identity, overlaps, relative_path, repository


def store_path():
    configured = os.environ.get('KRITIC_COORDINATION_DIR')
    if configured:
        return Path(configured)
    if os.name == 'nt':
        return Path('B:/AI/coordination') if Path('B:/').exists() else Path.home() / '.kritic/coordination'
    if Path('/mnt/b/AI').exists():
        return Path('/mnt/b/AI/coordination')
    return Path.home() / '.local/share/kritic/coordination'


@contextlib.contextmanager
def transaction():
    root = store_path()
    root.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(root / 'registry.sqlite3', timeout=5, isolation_level=None)
    db.row_factory = sqlite3.Row
    try:
        db.execute('CREATE TABLE IF NOT EXISTS actors (id TEXT PRIMARY KEY, repo TEXT NOT NULL, host TEXT NOT NULL, session TEXT NOT NULL, agent TEXT NOT NULL, worktree TEXT NOT NULL, seen REAL NOT NULL, ended INTEGER NOT NULL, generation INTEGER NOT NULL, declaration TEXT)')
        db.execute('CREATE TABLE IF NOT EXISTS intent_events (repo TEXT NOT NULL, actor TEXT NOT NULL, generation INTEGER NOT NULL, event TEXT NOT NULL, created REAL NOT NULL, payload TEXT NOT NULL)')
        db.execute('BEGIN IMMEDIATE')
        yield db
        db.execute('COMMIT')
    except BaseException:
        if db.in_transaction:
            db.execute('ROLLBACK')
        raise
    finally:
        db.close()


def actor_id(host, session, agent='main'):
    if host not in ('codex', 'claude', 'hermes') or not isinstance(session, str) or not session or len(session) > 512 or any(ord(c) < 32 for c in session):
        raise ValueError('host and actual nonempty session_id are required')
    if not isinstance(agent, str) or not agent or len(agent) > 512 or any(ord(c) < 32 for c in agent):
        raise ValueError('agent_id must be a nonempty string')
    return json.dumps([host, session, agent], separators=(',', ':'))


def actors(db, repo, include_ended=False):
    rows = []
    query = 'SELECT * FROM actors WHERE repo=? ORDER BY id' if include_ended else 'SELECT * FROM actors WHERE repo=? AND ended=0 ORDER BY id'
    for item in db.execute(query, (repo,)):
        row = dict(item)
        row['declaration'] = json.loads(row['declaration']) if row['declaration'] else None
        row['presence'] = 'recent' if time.time() - row['seen'] <= 1800 else 'suspect; claim retained'
        rows.append(row)
    return rows


def observe(db, host, session, agent, info):
    key = actor_id(host, session, agent)
    old = db.execute('SELECT * FROM actors WHERE id=?', (key,)).fetchone()
    if old is None:
        db.execute('INSERT INTO actors VALUES (?,?,?,?,?,?,?,0,1,NULL)',
                   (key, info['repo'], host, session, agent, info['worktree'], time.time()))
    elif old['repo'] != info['repo'] or identity(old['worktree']) != identity(info['worktree']) or old['ended']:
        audit(db, key, 'previous_intent_before_rebind', dict(old))
        db.execute('UPDATE actors SET repo=?,worktree=?,seen=?,ended=0,generation=generation+1,declaration=NULL WHERE id=?',
                   (info['repo'], info['worktree'], time.time(), key))
    else:
        db.execute('UPDATE actors SET seen=? WHERE id=?', (time.time(), key))
    return key


def audit(db, key, event, payload):
    row = db.execute('SELECT repo,generation FROM actors WHERE id=?', (key,)).fetchone()
    if row:
        db.execute('INSERT INTO intent_events VALUES (?,?,?,?,?,?)',
                   (row['repo'], key, row['generation'], event, time.time(), json.dumps(payload)))


def fields(value, required, optional=()):
    if not isinstance(value, dict) or set(value) - set(required) - set(optional) or set(required) - set(value):
        raise ValueError(f'Expected fields {list(required)}; optional {list(optional)}')


def text(value, name):
    if not isinstance(value, str) or not value.strip() or len(value) > 2000 or any(ord(c) < 32 for c in value):
        raise ValueError(f'{name} requires 1..2000 non-control characters')


def declare(request):
    fields(request, ('host', 'session_id', 'worktree', 'task', 'change_summary', 'scopes'), ('agent_id', 'access'))
    if request.get('access', 'edit') not in ('edit', 'read_only'):
        raise ValueError('access must be edit or read_only')
    text(request['task'], 'task')
    text(request['change_summary'], 'change_summary')
    scopes = request['scopes']
    if not isinstance(scopes, list) or not 1 <= len(scopes) <= 64:
        raise ValueError('Declare 1..64 exact files or directory prefixes with purposes')
    for scope in scopes:
        fields(scope, ('path', 'purpose'))
        scope['path'] = relative_path(scope['path'])
        text(scope['purpose'], 'scope purpose')
    info = repository(request['worktree'])
    if identity(info['worktree']) != identity(request['worktree']):
        raise ValueError('Declare the actual worktree root, not a subdirectory')
    if not all(contains(info['worktree'], scope['path'], scope['path'].rstrip('/')) for scope in scopes):
        raise ValueError('Declared scope escapes the worktree through a link or junction')
    key = actor_id(request['host'], request['session_id'], request.get('agent_id', 'main'))
    with transaction() as db:
        # A declaration never authorizes effects in a different repository.
        old = db.execute('SELECT repo,ended FROM actors WHERE id=?', (key,)).fetchone()
        if old and not old['ended'] and old['repo'] != info['repo']:
            raise ValueError('Session changed repository; observe it there before declaring')
        for other in actors(db, info['repo']):
            if other['id'] == key or not other['declaration'] or request.get('access') == 'read_only' or other['declaration'].get('access') == 'read_only':
                continue
            if identity(other['worktree']) == identity(info['worktree']):
                raise ValueError(f'Worktree reserved by {other["id"]}; use a separate worktree')
            windows_scope = bool(re.match(r'^[a-z]:/', identity(info['worktree'])))
            conflicts = [(a['path'], b['path']) for a in scopes for b in other['declaration']['scopes'] if overlaps(a['path'], b['path'], case_insensitive=windows_scope)]
            if conflicts:
                raise ValueError(f'Scope overlaps {other["id"]}: {conflicts}. Narrow or reassign scopes before editing; no claim was changed.')
        observe(db, request['host'], request['session_id'], request.get('agent_id', 'main'), info)
        declaration = dict(request, worktree=info['worktree'], declared_head=info['head'], declared_at=time.time())
        db.execute('UPDATE actors SET declaration=?,generation=generation+1 WHERE id=?', (json.dumps(declaration), key))
        generation = db.execute('SELECT generation FROM actors WHERE id=?', (key,)).fetchone()[0]
        audit(db, key, 'declared', declaration)
    return {'status': 'declared', 'actor_id': key, 'generation': generation, 'repo': info['repo'], 'declaration': declaration}


def release(request):
    fields(request, ('host', 'session_id', 'reason'), ('agent_id', 'expected_generation'))
    text(request['reason'], 'reason')
    if 'expected_generation' in request and (not isinstance(request['expected_generation'], int) or isinstance(request['expected_generation'], bool) or request['expected_generation'] < 1):
        raise ValueError('expected_generation must be a positive integer')
    key = actor_id(request['host'], request['session_id'], request.get('agent_id', 'main'))
    with transaction() as db:
        row = db.execute('SELECT generation FROM actors WHERE id=?', (key,)).fetchone()
        if not row:
            raise ValueError('Unknown actor; inspect status first')
        if request.get('expected_generation', row[0]) != row[0]:
            raise ValueError('Claim changed; inspect status again before releasing')
        db.execute('UPDATE actors SET ended=1,generation=generation+1 WHERE id=?', (key,))
        audit(db, key, 'released', {'reason': request['reason']})
    return {'status': 'released', 'actor_id': key, 'reason': request['reason'], 'worktree_action': 'none'}


def status(request):
    fields(request, ('worktree',), ('include_history',))
    if not isinstance(request.get('include_history', False), bool):
        raise ValueError('include_history must be boolean')
    info = repository(request['worktree'])
    with transaction() as db:
        active = actors(db, info['repo'])
        history = []
        if request.get('include_history'):
            for row in db.execute('SELECT * FROM intent_events WHERE repo=? ORDER BY created DESC LIMIT 64', (info['repo'],)):
                event = dict(row)
                event['payload'] = json.loads(event['payload'])
                history.append(event)
    return {'repo': info['repo'], 'worktree': info['worktree'], 'actors': active,
            'intent_history': history,
            'coverage': 'Registered hook events only; suspect claims are retained until session end or explicit release'}


def authorized(declaration, targets):
    return all(any(contains(declaration['worktree'], s['path'], target) for s in declaration['scopes']) for target in targets)

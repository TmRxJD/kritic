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
        db.execute('CREATE TABLE IF NOT EXISTS transfers (id TEXT PRIMARY KEY, repo TEXT NOT NULL, worktree TEXT NOT NULL, source TEXT NOT NULL, status TEXT NOT NULL, generation INTEGER NOT NULL, payload TEXT NOT NULL)')
        db.execute('CREATE TABLE IF NOT EXISTS actor_epochs (actor TEXT PRIMARY KEY, epoch INTEGER NOT NULL, boundary_epoch INTEGER, boundary_generation INTEGER)')
        # Kept separately for compatibility with older nine-column actor inserts.
        # The declaration remains the single authoritative intent/scope document.
        db.execute("CREATE TABLE IF NOT EXISTS claim_states (actor TEXT PRIMARY KEY, state TEXT NOT NULL CHECK(state IN ('active','quiescent','transferred','released','completed')))")
        db.execute('BEGIN IMMEDIATE')
        migrating = {row['id'] for row in db.execute('SELECT id FROM actors WHERE id NOT IN (SELECT actor FROM claim_states)')}
        db.execute("INSERT OR IGNORE INTO claim_states SELECT id,CASE WHEN declaration IS NOT NULL AND ended=0 THEN 'active' ELSE 'released' END FROM actors")
        # Recover pre-state-table transfer ownership from its durable intent log.
        # Runtime-end events are not ownership events and cannot override it.
        for row in db.execute("SELECT a.id,(SELECT event FROM intent_events e WHERE e.actor=a.id AND event IN ('ownership_accepted','ownership_transferred','released','declared') ORDER BY rowid DESC LIMIT 1) AS last_intent FROM actors a"):
            if row['id'] in migrating and row['last_intent'] in ('ownership_accepted','ownership_transferred'):
                state = 'active' if row['last_intent'] == 'ownership_accepted' else 'transferred'
                db.execute('UPDATE claim_states SET state=? WHERE actor=?',(state,row['id']))
        # Older adapters must fail closed rather than erase a durable assignment.
        db.execute("""CREATE TRIGGER IF NOT EXISTS preserve_durable_declaration BEFORE UPDATE OF declaration ON actors
            WHEN OLD.declaration IS NOT NULL AND NEW.declaration IS NULL AND EXISTS
            (SELECT 1 FROM claim_states WHERE actor=OLD.id AND state IN ('active','quiescent','transferred'))
            BEGIN SELECT RAISE(ABORT,'Durable ownership cannot be erased by session presence; use the matching coordinator'); END""")
        db.execute("""CREATE TRIGGER IF NOT EXISTS fence_transferred_writer BEFORE UPDATE OF declaration ON actors
            WHEN json_extract(NEW.declaration,'$.access') IS NOT 'read_only' AND EXISTS
            (SELECT 1 FROM claim_states WHERE actor=OLD.id AND state='transferred')
            BEGIN SELECT RAISE(ABORT,'Transferred source cannot reclaim the writer role; use an acknowledged transfer'); END""")
        # Old clients filter out ended actors. Enforce durable reservations at the
        # database boundary too; an old declaration must not create a second writer.
        db.execute("""CREATE VIEW IF NOT EXISTS durable_writer_conflicts AS
            SELECT w.id AS writer FROM actors w JOIN actors a ON a.id<>w.id AND a.repo=w.repo
            JOIN claim_states s ON s.actor=a.id
            WHERE w.declaration IS NOT NULL AND COALESCE(json_extract(w.declaration,'$.access'),'edit')='edit'
            AND s.state IN ('active','quiescent')
            AND a.declaration IS NOT NULL AND COALESCE(json_extract(a.declaration,'$.access'),'edit')='edit'
            AND (lower(replace(a.worktree,char(92),'/'))=lower(replace(w.worktree,char(92),'/')) OR EXISTS (
                SELECT 1 FROM json_each(a.declaration,'$.scopes') x,json_each(w.declaration,'$.scopes') y
                WHERE lower(json_extract(x.value,'$.path'))=lower(json_extract(y.value,'$.path'))
                OR (substr(json_extract(x.value,'$.path'),-1)='/' AND lower(substr(json_extract(y.value,'$.path'),1,length(json_extract(x.value,'$.path'))))=lower(json_extract(x.value,'$.path')))
                OR (substr(json_extract(y.value,'$.path'),-1)='/' AND lower(substr(json_extract(x.value,'$.path'),1,length(json_extract(y.value,'$.path'))))=lower(json_extract(y.value,'$.path')))))""")
        db.execute("""CREATE TRIGGER IF NOT EXISTS durable_writer_insert AFTER INSERT ON actors
            WHEN EXISTS (SELECT 1 FROM durable_writer_conflicts WHERE writer=NEW.id)
            BEGIN SELECT RAISE(ABORT,'Durable writer reservation conflicts; use the matching coordinator'); END""")
        db.execute("""CREATE TRIGGER IF NOT EXISTS durable_writer_update AFTER UPDATE OF declaration,worktree,repo ON actors
            WHEN EXISTS (SELECT 1 FROM durable_writer_conflicts WHERE writer=NEW.id)
            BEGIN SELECT RAISE(ABORT,'Durable writer reservation conflicts; use the matching coordinator'); END""")
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
    query = 'SELECT a.*,s.state AS claim_state FROM actors a LEFT JOIN claim_states s ON s.actor=a.id WHERE a.repo=?'
    if not include_ended:
        query += " AND (a.ended=0 OR s.state IN ('active','quiescent'))"
    query += ' ORDER BY a.id'
    for item in db.execute(query, (repo,)):
        row = dict(item)
        row['declaration'] = json.loads(row['declaration']) if row['declaration'] else None
        row['presence'] = 'recent' if time.time() - row['seen'] <= 1800 else 'suspect; claim retained'
        if row['ended'] and row['claim_state'] in ('active','quiescent'):
            row['presence'] = 'runtime ended; durable claim retained'
        rows.append(row)
    return rows


def observe(db, host, session, agent, info):
    key = actor_id(host, session, agent)
    old = db.execute('SELECT * FROM actors WHERE id=?', (key,)).fetchone()
    if old is None:
        db.execute('INSERT INTO actors VALUES (?,?,?,?,?,?,?,0,1,NULL)',
                   (key, info['repo'], host, session, agent, info['worktree'], time.time()))
        db.execute("INSERT INTO claim_states VALUES (?,'released')",(key,))
    elif old['repo'] != info['repo'] or identity(old['worktree']) != identity(info['worktree']):
        state = claim_state(db,key)
        if state in ('active','quiescent'):
            raise ValueError('Current durable work assignment is still owned; explicitly release or transfer it before rebinding this actor')
        audit(db, key, 'previous_intent_before_rebind', dict(old))
        db.execute("UPDATE claim_states SET state='released' WHERE actor=?",(key,))
        db.execute('UPDATE actors SET repo=?,worktree=?,seen=?,ended=0,declaration=NULL WHERE id=?',
                   (info['repo'], info['worktree'], time.time(),key))
    else:
        if claim_state(db,key) in ('released','completed'):
            db.execute('UPDATE actors SET declaration=NULL,seen=?,ended=0 WHERE id=?',(time.time(),key))
        else:
            db.execute('UPDATE actors SET seen=?,ended=0 WHERE id=?', (time.time(), key))
    return key


def transfer_fence(db,key,info):
    row = db.execute('SELECT repo,worktree FROM actors WHERE id=?',(key,)).fetchone()
    return claim_state(db,key) == 'transferred' and row and row['repo'] == info['repo'] and identity(row['worktree']) == identity(info['worktree'])


def claim_state(db,key):
    row = db.execute('SELECT state FROM claim_states WHERE actor=?',(key,)).fetchone()
    return row['state'] if row else 'released'


def pending_transfer(db,key):
    return db.execute("SELECT id FROM transfers WHERE source=? AND status IN ('prepared','bound','launch_unknown') LIMIT 1",(key,)).fetchone()


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
        if pending_transfer(db,key):
            raise ValueError('Cancel or finish the pending transfer before changing source intent or scopes')
        fenced = bool(transfer_fence(db,key,info))
        if request.get('access','edit') == 'edit' and fenced:
            raise ValueError('This source session transferred its write ownership; resume in the successor or arrange an explicit transfer back')
        reserved = db.execute("SELECT id,source FROM transfers WHERE repo=? AND worktree=? AND status IN ('prepared','bound','launch_unknown') LIMIT 1",(info['repo'],info['worktree'])).fetchone()
        if request.get('access','edit') == 'edit' and reserved:
            raise ValueError(f'Worktree has a pending ownership transfer {reserved["id"]}; preserve its reservation')
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
        db.execute('UPDATE claim_states SET state=? WHERE actor=?',('transferred' if fenced else 'active',key))
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
        if pending_transfer(db,key):
            raise ValueError('Cancel or finish the pending transfer before releasing its reservation')
        if claim_state(db,key) == 'transferred':
            return {'status':'transferred','actor_id':key,'worktree_action':'none','reason':'The successor owns the write assignment; source presence cannot release it'}
        row = db.execute('SELECT generation FROM actors WHERE id=?', (key,)).fetchone()
        if not row:
            raise ValueError('Unknown actor; inspect status first')
        if request.get('expected_generation', row[0]) != row[0]:
            raise ValueError('Claim changed; inspect status again before releasing')
        db.execute("UPDATE claim_states SET state='released' WHERE actor=?",(key,))
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
            'coverage': 'Registered hook events only; runtime end changes presence, not durable ownership. Explicit release or acknowledged transfer closes editing claims'}


def authorized(declaration, targets):
    return all(any(contains(declaration['worktree'], s['path'], target) for s in declaration['scopes']) for target in targets)

"""Cross-agent cache/leases. Success freshness and failed-attempt backoff differ."""
import contextlib
import json
from pathlib import Path
import sqlite3
import time


@contextlib.contextmanager
def transaction(policy):
    root=Path(policy['state_dir'])
    root.mkdir(parents=True,exist_ok=True)
    db=sqlite3.connect(root/'updates.sqlite3',timeout=3,isolation_level=None)
    db.row_factory=sqlite3.Row
    try:
        db.execute('CREATE TABLE IF NOT EXISTS tools (id TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,checked_at REAL,metadata TEXT,error TEXT,retry_after REAL NOT NULL DEFAULT 0,lease TEXT,lease_until REAL NOT NULL DEFAULT 0,artifact TEXT,stage_error TEXT)')
        db.execute('CREATE TABLE IF NOT EXISTS sessions (host TEXT NOT NULL,session TEXT NOT NULL,tool TEXT NOT NULL,binding TEXT NOT NULL,PRIMARY KEY(host,session,tool))')
        db.execute('CREATE TABLE IF NOT EXISTS notices (host TEXT NOT NULL,session TEXT NOT NULL,tool TEXT NOT NULL,signature TEXT NOT NULL,PRIMARY KEY(host,session,tool))')
        db.execute('CREATE TABLE IF NOT EXISTS probes (id TEXT PRIMARY KEY,identity TEXT NOT NULL,observed_at REAL NOT NULL,sample TEXT NOT NULL)')
        db.execute('BEGIN IMMEDIATE')
        yield db
        db.execute('COMMIT')
    except BaseException:
        if db.in_transaction: db.execute('ROLLBACK')
        raise
    finally: db.close()


def row(db,identifier,key):
    existing=db.execute('SELECT * FROM tools WHERE id=?',(identifier,)).fetchone()
    if existing and existing['fingerprint']!=key:
        db.execute('DELETE FROM tools WHERE id=?',(identifier,))
        existing=None
    if not existing:
        db.execute('INSERT INTO tools(id,fingerprint) VALUES (?,?)',(identifier,key))
        existing=db.execute('SELECT * FROM tools WHERE id=?',(identifier,)).fetchone()
    return dict(existing)


def read(policy,identifier):
    with transaction(policy) as db:
        result=db.execute('SELECT * FROM tools WHERE id=?',(identifier,)).fetchone()
    if not result: return None
    result=dict(result)
    for field in ('metadata','artifact'):
        if result[field]: result[field]=json.loads(result[field])
    if result['checked_at'] is not None: result['age_seconds']=max(0,time.time()-result['checked_at'])
    return result

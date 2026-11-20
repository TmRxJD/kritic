"""Explicit successor recovery when a source cannot cooperate. Never edits Git/files.

Authorization is a recorded user instruction supplied by the caller, not an
authentication boundary. Missing heartbeats or model opinions never authorize it.
"""
import hashlib
import json
import re
import time

from paths import identity, overlaps, repository
from registry import actor_id, actors, audit, claim_state, fields, text, transaction
from transfers import snapshot


GAPS = [
    'The unavailable source supplied no fresh handoff or turn-boundary acknowledgement.',
    'Conversation-only requirements and fixture provenance remain unverified; reconstruct them before relying on them.',
    'Hooks coordinate enrolled clients; they cannot stop already-running commands or unhooked filesystem writes.',
]


def schema():
    string = {'type':'string','minLength':1,'maxLength':2000}
    common = {'host':{'enum':['codex','claude','hermes']},'session_id':string,'agent_id':string}
    authorization = {'type':'object','additionalProperties':False,
        'required':['kind','user_request','reason'], 'properties':{
            'kind':{'const':'user_request'},'user_request':string,'reason':string}}
    inspect = dict(common,phase={'const':'inspect'},worktree=string,source_actor_id=string,
        expected_generation={'type':'integer','minimum':1},authorization=authorization)
    accept = dict(common,phase={'const':'accept'},plan={'type':'object'},
        plan_sha256={'type':'string','pattern':'^[0-9a-f]{64}$'},acknowledge_context_gaps={'const':True})
    return {'oneOf':[
        {'type':'object','additionalProperties':False,'required':['phase','host','session_id','worktree','source_actor_id','expected_generation','authorization'],'properties':inspect},
        {'type':'object','additionalProperties':False,'required':['phase','host','session_id','plan','plan_sha256','acknowledge_context_gaps'],'properties':accept}],
        'description':'Explicit user-authorized unavailable-owner recovery. Inspect changes no claims; accept inherits one exact assignment and fences its old writer. User authorization is self-attested, not authenticated.'}


def digest(plan):
    return hashlib.sha256(json.dumps(plan,sort_keys=True,separators=(',',':')).encode()).hexdigest()


def make_plan(db, key, worktree, source_id, generation, authorization):
    fields(authorization,('kind','user_request','reason'))
    if authorization['kind'] != 'user_request':
        raise ValueError('Recovery requires an explicit user request, never inferred inactivity')
    text(authorization['user_request'],'authorization.user_request')
    text(authorization['reason'],'authorization.reason')
    info = repository(worktree)
    if identity(info['worktree']) != identity(worktree):
        raise ValueError('Recover the actual worktree root, not a subdirectory')
    source = db.execute('SELECT * FROM actors WHERE id=?',(source_id,)).fetchone()
    if not source or source['repo'] != info['repo'] or identity(source['worktree']) != identity(info['worktree']):
        raise ValueError('Source ownership does not belong to this exact repository/worktree')
    if not isinstance(generation,int) or isinstance(generation,bool) or generation != source['generation']:
        raise ValueError('Source ownership generation changed; inspect status and recovery again')
    declaration = json.loads(source['declaration'] or 'null')
    state = claim_state(db,source_id)
    if not declaration or declaration.get('access','edit') != 'edit' or state not in ('active','quiescent'):
        raise ValueError('Source has no recoverable editing assignment')
    caller = json.loads(key)
    if source['host'] == caller[0] and source['session'] == caller[1]:
        raise ValueError('Recovery requires a different actual session or host, not another agent in the source session')
    existing = db.execute('SELECT * FROM actors WHERE id=?',(key,)).fetchone()
    if existing:
        if existing['repo'] != info['repo'] or identity(existing['worktree']) != identity(info['worktree']):
            raise ValueError('Successor already belongs to another worktree; preserve its assignment')
        current = json.loads(existing['declaration'] or 'null')
        if claim_state(db,key) in ('active','quiescent','transferred') and current:
            raise ValueError('Successor already has a durable assignment or is fenced; resolve it explicitly')
    for other in actors(db,info['repo']):
        if other['id'] in (source_id,key) or not other['declaration'] or other['declaration'].get('access','edit') != 'edit':
            continue
        if identity(other['worktree']) == identity(info['worktree']) or any(
                overlaps(a['path'],b['path'],case_insensitive=bool(re.match(r'^[a-z]:/',identity(info['worktree'])))) for a in declaration['scopes'] for b in other['declaration']['scopes']):
            raise ValueError('Another writer conflicts with the inherited assignment; reconcile it separately')
    pending = [dict(row) for row in db.execute(
        "SELECT id,source,status,generation FROM transfers WHERE repo=? AND worktree=? AND status IN ('prepared','bound','launch_unknown') ORDER BY id",
        (info['repo'],info['worktree']))]
    if any(row['source'] != source_id for row in pending):
        raise ValueError('Another source has a pending transfer in this tree')
    epoch = db.execute('SELECT epoch FROM actor_epochs WHERE actor=?',(source_id,)).fetchone()
    return {'version':1,'repo':info['repo'],'worktree':info['worktree'],
        'source_actor_id':source_id,'source_generation':generation,'source_claim_state':state,
        'source_epoch':epoch['epoch'] if epoch else None,
        'source_seen':source['seen'],
        'source_declaration':declaration,'successor_actor_id':key,
        'successor_generation':existing['generation'] if existing else None,
        'pending_transfers':pending,'authorization':authorization,'snapshot':snapshot(info['worktree']),
        'context_gaps':GAPS}


def recover(request):
    phase = request.get('phase')
    common = ('phase','host','session_id')
    extras = {'inspect':('worktree','source_actor_id','expected_generation','authorization'),
              'accept':('plan','plan_sha256','acknowledge_context_gaps')}
    if phase not in extras:
        raise ValueError('recover phase must be inspect or accept; use action=schema')
    fields(request,common+extras[phase],('agent_id',))
    key = actor_id(request['host'],request['session_id'],request.get('agent_id','main'))
    with transaction() as db:
        if phase == 'inspect':
            plan = make_plan(db,key,request['worktree'],request['source_actor_id'],request['expected_generation'],request['authorization'])
            return {'status':'review_required','plan':plan,'plan_sha256':digest(plan),
                'writer_transferred':False,'worktree_action':'none','next_step':'Review inherited task/scopes, dirty-file inventory and context gaps; accept this exact plan with your actual identity.'}
        plan = request['plan']
        if not isinstance(plan,dict) or request['acknowledge_context_gaps'] is not True:
            raise ValueError('A recovery plan and explicit context-gap acknowledgement are required')
        if plan.get('successor_actor_id') != key or request['plan_sha256'] != digest(plan):
            raise ValueError('Recovery plan/hash belongs to another successor or was changed')
        live = make_plan(db,key,plan.get('worktree',''),plan.get('source_actor_id',''),plan.get('source_generation'),plan.get('authorization',{}))
        if live != plan:
            raise ValueError('Recovery evidence changed; inspect again without discarding work')
        source = plan['source_actor_id']
        original = plan['source_declaration']
        source_decl = dict(original,access='read_only',transferred_to=key)
        destination = dict(original,host=request['host'],session_id=request['session_id'],
            agent_id=request.get('agent_id','main'),access='edit',transferred_from=source,
            recovery_context_unverified=True)
        # Fence the source before installing the successor. Any later error rolls
        # back all claims, pending transfers and audit records in this transaction.
        db.execute('UPDATE actors SET declaration=?,generation=generation+1 WHERE id=?',(json.dumps(source_decl),source))
        db.execute("UPDATE claim_states SET state='transferred' WHERE actor=?",(source,))
        for pending in plan['pending_transfers']:
            db.execute("UPDATE transfers SET status='cancelled',generation=generation+1 WHERE id=?",(pending['id'],))
        if plan['successor_generation'] is None:
            db.execute('INSERT INTO actors VALUES (?,?,?,?,?,?,?,0,1,?)',
                (key,plan['repo'],request['host'],request['session_id'],request.get('agent_id','main'),plan['worktree'],time.time(),json.dumps(destination)))
        else:
            db.execute('UPDATE actors SET ended=0,declaration=?,generation=generation+1,seen=? WHERE id=?',(json.dumps(destination),time.time(),key))
        db.execute("INSERT INTO claim_states VALUES (?,'active') ON CONFLICT(actor) DO UPDATE SET state='active'",(key,))
        payload = {'source':source,'successor':key,'plan_sha256':request['plan_sha256'],
                   'authorization':plan['authorization'],'snapshot_sha256':plan['snapshot']['sha256'],
                   'invalidated_transfers':[p['id'] for p in plan['pending_transfers']],'context_gaps':GAPS}
        audit(db,source,'ownership_transferred',dict(payload,recovery=True))
        audit(db,key,'ownership_accepted',dict(payload,recovery=True))
        generation = db.execute('SELECT generation FROM actors WHERE id=?',(key,)).fetchone()[0]
        return {'status':'accepted','actor_id':key,'owner_generation':generation,'worktree':plan['worktree'],
            'task':destination['task'],'scopes':destination['scopes'],'writer_transferred':True,
            'worktree_action':'none','context_gaps':GAPS,'authorization_authenticated':False}

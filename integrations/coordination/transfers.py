"""Generation-checked same-worktree succession. No Git/file mutations or silent takeovers."""
import hashlib
import json
from pathlib import Path
import secrets
import time

from paths import git, identity, local_path, relative_path, repository
from registry import actor_id, actors, audit, claim_state, fields, pending_transfer, transaction


def schema():
    """Exact phase schemas, discovered on demand rather than in every tool call."""
    text = {'type':'string','minLength':1,'maxLength':2000}
    generation = {'type':'integer','minimum':1}
    identity_fields = {'host':{'enum':['codex','claude','hermes']},'session_id':text,'agent_id':text}
    pin = {'type':'object','additionalProperties':False,'required':['path','sha256'],
           'properties':{'path':text,'sha256':{'type':'string','pattern':'^[0-9a-f]{64}$'}}}
    identifiers = {'type':'array','uniqueItems':True,'items':dict(text,maxLength=512)}
    phases = {
        'prepare':{'expected_generation':generation,'handoff_report':pin},
        'bind':{'transfer_id':text,'expected_transfer_generation':generation,'destination':{
            'type':'object','additionalProperties':False,'required':['host','session_id'],'properties':identity_fields}},
        'accept':{'transfer_id':text,'expected_transfer_generation':generation,'handoff_sha256':pin['properties']['sha256'],
                  'accepted_user_event_ids':identifiers,'accepted_fact_ids':identifiers,'accepted_fixture_ids':identifiers},
        'cancel':{'transfer_id':text,'expected_transfer_generation':generation,'reason':text},
        'status':{'transfer_id':text}}
    return {'oneOf':[{'type':'object','additionalProperties':False,
        'required':['phase','host','session_id',*properties],
        'properties':dict(identity_fields,phase={'const':phase},**properties)} for phase,properties in phases.items()]}


def read_pin(reference, limit=1_000_000):
    fields(reference,('path','sha256'))
    given = local_path(reference['path'])
    if not given.is_absolute():
        raise ValueError('Transfer pins must use explicit absolute local paths')
    path = given.resolve()
    if not path.is_file() or path.stat().st_size > limit:
        raise ValueError('Pin must name a bounded existing local file')
    digest = hashlib.sha256()
    data = bytearray()
    with path.open('rb') as stream:
        while chunk := stream.read(65536):
            if len(data)+len(chunk) > limit:
                raise ValueError('Pin exceeds its read budget')
            data.extend(chunk)
            digest.update(chunk)
    if digest.hexdigest() != reference['sha256']:
        raise ValueError(f'Stale transfer evidence: {path}')
    return bytes(data)


def snapshot(root):
    info = repository(root)
    dirty = git(root,'status','--porcelain=v1','-z','--untracked-files=all')
    index = git(root,'diff','--cached','--raw','--no-ext-diff','--no-textconv','-z','HEAD','--')
    tokens = iter(dirty.split('\0'))
    paths = set()
    for token in tokens:
        if not token:
            continue
        paths.add(relative_path(token[3:]))
        if 'R' in token[:2] or 'C' in token[:2]:
            paths.add(relative_path(next(tokens,'')))
    if len(paths) > 256:
        raise ValueError('Transfer snapshot exceeds 256 dirty/untracked paths')
    pins = []
    remaining = 128_000_000
    for name in sorted(paths):
        path = Path(info['worktree'])/name
        if not path.exists():
            pins.append({'path':name,'deleted':True})
            continue
        if not path.resolve().is_relative_to(Path(info['worktree']).resolve()) or not path.is_file():
            raise ValueError('Dirty links/directories escaping the worktree need explicit resolution before succession')
        size = path.stat().st_size
        if size > 20_000_000 or size > remaining:
            raise ValueError('Dirty content exceeds transfer snapshot budget')
        remaining -= size
        content = path.read_bytes()
        if len(content) != size:
            raise ValueError('Dirty file changed while snapshotting')
        pins.append({'path':name,'mode':path.lstat().st_mode,'sha256':hashlib.sha256(content).hexdigest()})
    value = {'head':info['head'],'dirty':dirty,'index':index,'files':pins}
    value['sha256'] = hashlib.sha256(json.dumps(value,sort_keys=True,separators=(',',':')).encode()).hexdigest()
    return value


def checked_report(reference, row):
    report = json.loads(read_pin(reference,4_000_000))
    if report.get('status') != 'covered' or not report.get('ready_for_controller_review') or report.get('coverage_gaps') or report.get('findings'):
        raise ValueError('Handoff report has gaps; covered controller-review evidence is required')
    if report.get('source_session_id') != row['session'] or identity(report.get('worktree','')) != identity(row['worktree']):
        raise ValueError('Handoff report belongs to another session/worktree')
    declaration = json.loads(row['declaration'])
    if sorted(relative_path(p) for p in report.get('scope',[])) != sorted(s['path'] for s in declaration['scopes']):
        raise ValueError('Handoff scope must exactly match the source write declaration')
    read_pin({'path':report['handoff_path'],'sha256':report['handoff_sha256']})
    pins = report.get('resume_pins')
    if not isinstance(pins,list) or not 1 <= len(pins) <= 512:
        raise ValueError('Handoff requires its bounded resume evidence pins')
    total = 0
    for pin in pins:
        total += local_path(pin['path']).stat().st_size
        if total > 128_000_000:
            raise ValueError('Resume evidence exceeds 128 MB transfer budget')
        read_pin(pin,64_000_000)
    for key in ('required_user_event_ids','required_fact_ids','required_fixture_ids'):
        ids = report.get(key)
        if not isinstance(ids,list) or any(not isinstance(item,str) or not item or len(item)>512 for item in ids) or len(ids) != len(set(ids)) or (key != 'required_fixture_ids' and not ids):
            raise ValueError(f'Invalid handoff acknowledgement inventory: {key}')
    report.pop('curated_context',None)
    return report


def check_epoch(db, source, data):
    epoch = db.execute('SELECT * FROM actor_epochs WHERE actor=?',(source['id'],)).fetchone()
    if not epoch or epoch['boundary_epoch'] != epoch['epoch'] or epoch['boundary_generation'] != source['generation']:
        raise ValueError('No current observed turn boundary; finish the turn and capture a new handoff')
    if data is not None and epoch['epoch'] != data['source_epoch']:
        raise ValueError('A new user/work event arrived after transfer preparation; refresh the handoff')
    return epoch['epoch']


def transfer(request):
    phase = request.get('phase')
    common = ('phase','host','session_id')
    optional = ('agent_id',)
    extras = {'prepare':('expected_generation','handoff_report'),
              'bind':('transfer_id','expected_transfer_generation','destination'),
              'accept':('transfer_id','expected_transfer_generation','handoff_sha256','accepted_user_event_ids','accepted_fact_ids','accepted_fixture_ids'),
              'cancel':('transfer_id','expected_transfer_generation','reason'),
              'status':('transfer_id',)}
    if phase not in extras:
        raise ValueError('transfer phase must be prepare, bind, accept, cancel or status')
    fields(request,common+extras[phase],optional)
    key = actor_id(request['host'],request['session_id'],request.get('agent_id','main'))
    with transaction() as db:
        if phase == 'prepare':
            source = db.execute('SELECT * FROM actors WHERE id=?',(key,)).fetchone()
            if not source or source['ended'] or claim_state(db,key) != 'active' or not source['declaration'] or json.loads(source['declaration']).get('access','edit') != 'edit':
                raise ValueError('Only the current declared editing owner can prepare succession')
            if not isinstance(request['expected_generation'],int) or isinstance(request['expected_generation'],bool) or request['expected_generation'] != source['generation']:
                raise ValueError('Source ownership generation changed')
            if pending_transfer(db,key):
                raise ValueError('A source transfer is already pending; reconcile it rather than launching twice')
            epoch = check_epoch(db,source,None)
            report = checked_report(request['handoff_report'],source)
            state = snapshot(source['worktree'])
            if state['head'] != report.get('workspace_revision'):
                raise ValueError('Workspace revision differs from handoff evidence')
            data = {'source_generation':source['generation'],'source_epoch':epoch,'source_declaration':json.loads(source['declaration']),
                    'handoff_report':request['handoff_report'],'snapshot':state,'report':report,'destination':None,'created_at':time.time()}
            identifier = secrets.token_hex(16)
            db.execute('INSERT INTO transfers VALUES (?,?,?,?,?,?,?)',(identifier,source['repo'],source['worktree'],key,'prepared',1,json.dumps(data)))
            db.execute("UPDATE claim_states SET state='quiescent' WHERE actor=?",(key,))
            audit(db,key,'transfer_prepared',{'transfer_id':identifier,'handoff_sha256':report['handoff_sha256']})
            return {'status':'prepared','transfer_id':identifier,'generation':1,'handoff_sha256':report['handoff_sha256'],'worktree':source['worktree'],'writer_transferred':False}
        row = db.execute('SELECT * FROM transfers WHERE id=?',(request['transfer_id'],)).fetchone()
        if not row:
            raise ValueError('Unknown transfer; never infer ownership from a missing heartbeat')
        data = json.loads(row['payload'])
        if key not in (row['source'],data.get('destination')):
            raise ValueError('Caller is neither the recorded source nor bound successor')
        if phase == 'status':
            return {'status':row['status'],'transfer_id':row['id'],'generation':row['generation'],'destination':data.get('destination'),'writer_transferred':row['status']=='accepted'}
        expected = request['expected_transfer_generation']
        if not isinstance(expected,int) or isinstance(expected,bool) or expected != row['generation']:
            raise ValueError('Transfer generation changed; reconcile status before retrying')
        if phase == 'cancel':
            if key != row['source'] or row['status'] not in ('prepared','bound') or not isinstance(request['reason'],str) or not request['reason'].strip() or len(request['reason']) > 2000:
                raise ValueError('Only the source can cancel an unaccepted transfer with a reason')
            db.execute("UPDATE transfers SET status='cancelled',generation=generation+1 WHERE id=?",(row['id'],))
            db.execute("UPDATE claim_states SET state='active' WHERE actor=?",(key,))
            audit(db,key,'transfer_cancelled',{'transfer_id':row['id'],'reason':request['reason']})
            return {'status':'cancelled','writer_transferred':False}
        source = db.execute('SELECT * FROM actors WHERE id=?',(row['source'],)).fetchone()
        if not source or source['generation'] != data['source_generation']:
            raise ValueError('Source ownership changed after preparation')
        check_epoch(db,source,data)
        report = checked_report(data['handoff_report'],source)
        if snapshot(row['worktree']) != data['snapshot']:
            raise ValueError('Workspace/index/dirty content changed after preparation; do not overwrite it')
        if phase == 'bind':
            if key != row['source'] or row['status'] != 'prepared':
                raise ValueError('Only the source can bind one observed fresh-session launch receipt')
            destination = request['destination']
            fields(destination,('host','session_id'),('agent_id',))
            target = actor_id(destination['host'],destination['session_id'],destination.get('agent_id','main'))
            if destination['host'] == source['host'] and destination['session_id'] == source['session']:
                raise ValueError('A fresh successor must have a different actual session identity')
            data['destination'] = target
            db.execute("UPDATE transfers SET status='bound',generation=generation+1,payload=? WHERE id=?",(json.dumps(data),row['id']))
            return {'status':'bound','transfer_id':row['id'],'generation':row['generation']+1,'destination':target,'writer_transferred':False}
        if key != data.get('destination') or row['status'] != 'bound':
            raise ValueError('Only the bound successor can acknowledge and accept')
        if request['handoff_sha256'] != report['handoff_sha256']:
            raise ValueError('Successor acknowledged the wrong handoff hash')
        for supplied,required in [('accepted_user_event_ids','required_user_event_ids'),('accepted_fact_ids','required_fact_ids'),('accepted_fixture_ids','required_fixture_ids')]:
            values = request[supplied]
            if not isinstance(values,list) or len(values) != len(set(values)) or set(values) != set(report[required]):
                raise ValueError(f'Incomplete successor acknowledgement: {supplied}')
        for other in actors(db,row['repo']):
            if other['id'] not in (row['source'],key) and other['declaration'] and other['declaration'].get('access','edit') == 'edit' and identity(other['worktree']) == identity(row['worktree']):
                raise ValueError('Another writer claimed the worktree; reconcile before succession')
        source_decl = dict(data['source_declaration'],access='read_only',transferred_to=key)
        destination_decl = dict(data['source_declaration'],host=request['host'],session_id=request['session_id'],agent_id=request.get('agent_id','main'),access='edit',transferred_from=row['source'])
        db.execute('UPDATE actors SET declaration=?,generation=generation+1 WHERE id=?',(json.dumps(source_decl),row['source']))
        db.execute("UPDATE claim_states SET state='transferred' WHERE actor=?",(row['source'],))
        existing = db.execute('SELECT * FROM actors WHERE id=?',(key,)).fetchone()
        if existing and (existing['repo'] != row['repo'] or identity(existing['worktree']) != identity(row['worktree'])):
            raise ValueError('Successor is bound to a different worktree; no claims changed')
        if existing:
            if existing['declaration'] and json.loads(existing['declaration']).get('access','edit') == 'edit':
                raise ValueError('Successor already owns an editing assignment; preserve it')
            db.execute("UPDATE claim_states SET state='active' WHERE actor=?",(key,))
            db.execute('UPDATE actors SET ended=0,declaration=?,generation=generation+1,seen=? WHERE id=?',(json.dumps(destination_decl),time.time(),key))
        else:
            db.execute('INSERT INTO actors VALUES (?,?,?,?,?,?,?,0,1,?)',(key,row['repo'],request['host'],request['session_id'],request.get('agent_id','main'),row['worktree'],time.time(),json.dumps(destination_decl)))
        db.execute("INSERT INTO claim_states VALUES (?,'active') ON CONFLICT(actor) DO UPDATE SET state='active'",(key,))
        db.execute("UPDATE transfers SET status='accepted',generation=generation+1 WHERE id=?",(row['id'],))
        audit(db,row['source'],'ownership_transferred',{'transfer_id':row['id'],'destination':key})
        audit(db,key,'ownership_accepted',{'transfer_id':row['id'],'source':row['source']})
        generation = db.execute('SELECT generation FROM actors WHERE id=?',(key,)).fetchone()[0]
        return {'status':'accepted','transfer_id':row['id'],'generation':row['generation']+1,'actor_id':key,'owner_generation':generation,'worktree':row['worktree'],'scopes':destination_decl['scopes'],'writer_transferred':True,'worktree_action':'none'}

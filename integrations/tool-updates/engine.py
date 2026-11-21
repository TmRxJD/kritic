"""Cached update checks and required staging, independent of critic models."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess  # nosec B404
import sys
import time

from config import fingerprint
import providers
import state


def selected_bindings(tool):
    results=[]
    for path in tool['installed'].get('client_bindings',[]):
        source=Path(path)
        if not source.is_file(): raise ValueError('Registered client binding file is missing: '+path)
        text=source.read_text(encoding='utf8')
        if source.suffix=='.toml':
            import tomllib
            data=tomllib.loads(text)
        elif source.suffix=='.json': data=json.loads(text)
        else:
            import yaml
            data=yaml.safe_load(text)
        servers=data.get('mcp_servers',data.get('mcpServers',{}))
        selected=servers.get(tool['installed'].get('server',tool['id']))
        if not isinstance(selected,dict): raise ValueError('Expected tool registration is missing from '+path)
        results.append((path,{k:selected[k] for k in ('command','args','env','enabled','disabled') if k in selected}))
    return results


def binding(tool):
    value=[fingerprint(tool)]
    value.extend(selected_bindings(tool))
    return hashlib.sha256(json.dumps(value,sort_keys=True).encode()).hexdigest()


def canonical_scripts(tool):
    root=Path(tool['installed']['path']).parent
    data=json.loads((root/'package.json').read_text(encoding='utf8'))
    bins=data.get('bin',{})
    entries=list(bins.values()) if isinstance(bins,dict) else ([bins] if isinstance(bins,str) else [])
    if tool['installed'].get('entrypoint'): entries.append(tool['installed']['entrypoint'])
    if not entries: raise ValueError('Npm entrypoint is unverified; configure the actual installed entrypoint')
    result=[]
    for entry in entries:
        if not isinstance(entry,str): raise ValueError('Invalid npm entrypoint')
        path=(root/entry).resolve()
        if not path.is_relative_to(root.resolve()) or not path.is_file(): raise ValueError('Canonical npm entrypoint missing/escaping installation')
        result.append(path)
    return result


def validate_bindings(tool):
    probe=tool['installed']
    for path,selected in selected_bindings(tool):
        command=selected.get('command')
        if not isinstance(command,str): raise ValueError('Client registration lacks a verifiable executable: '+path)
        executable=Path(shutil.which(command) or command).resolve()
        if not executable.is_file(): raise ValueError('Registered executable is missing: '+path)
        args=selected.get('args',[])
        if probe['kind']=='npm':
            if probe.get('runtime') and executable!=Path(probe['runtime']).resolve():
                raise ValueError('Registered Node runtime differs from the canonical compatible runtime: '+path)
            if not args or not Path(args[0]).is_absolute() or Path(args[0]).resolve() not in canonical_scripts(tool):
                raise ValueError('Registered npm script differs from the canonical installation: '+path)
        elif probe['kind']=='python_metadata':
            if not probe.get('runtime') or executable.parent!=Path(probe['runtime']).parent:
                raise ValueError('Registered Python environment differs from canonical installation: '+path)
        elif probe['kind']=='command' and executable!=Path(probe['argv'][0]).resolve():
            raise ValueError('Registered executable differs from canonical installation: '+path)


def observed_installation(policy,tool,now,interval,force=False):
    """Reuse an observed version, never rank versions by filesystem timestamps.

    Small package metadata is content-hashed. Executable identity/stat changes
    invalidate a bounded probe cache; periodic probing remains mandatory. This
    is change detection, not an adversarial filesystem integrity attestation.
    """
    probe=tool['installed']; parts=[fingerprint(tool)]
    paths=[probe['argv'][0]] if probe['kind']=='command' else [probe['path']]
    if probe.get('runtime'): paths.append(probe['runtime'])
    for name in paths:
        path=Path(name); info=path.stat()
        if name==probe.get('path'):
            with path.open('rb') as stream: content=stream.read(1024**2+1)
            if len(content)>1024**2: raise ValueError('Installed package metadata exceeds1MiB')
            value=hashlib.sha256(content).hexdigest()
        else: value=[info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns,info.st_ctime_ns]
        parts.append([name,value])
    identity=hashlib.sha256(json.dumps(parts,sort_keys=True).encode()).hexdigest()
    with state.transaction(policy) as db:
        old=db.execute('SELECT * FROM probes WHERE id=?',(tool['id'],)).fetchone()
        if old and old['identity']==identity and not force:
            sample=json.loads(old['sample']); ttl=policy['retry_seconds'] if 'error' in sample else interval
            if 0<=now-old['observed_at']<ttl:
                if 'error' in sample: raise ValueError(sample['error'])
                return sample
    try: sample=providers.installed(tool)
    except Exception as error: sample={'error':str(error)}
    with state.transaction(policy) as db:
        db.execute('INSERT INTO probes VALUES (?,?,?,?) ON CONFLICT(id) DO UPDATE SET identity=excluded.identity,observed_at=excluded.observed_at,sample=excluded.sample',
            (tool['id'],identity,now,json.dumps(sample)))
    if 'error' in sample: raise ValueError(sample['error'])
    return sample


def session_start(config,host,session):
    if not config['policy']['enabled']: return
    with state.transaction(config['policy']) as db:
        for tool in config['tool']:
            db.execute('INSERT OR IGNORE INTO sessions VALUES (?,?,?,?)',(host,session,tool['id'],binding(tool)))


def result(tool,status,allow,**kwargs):
    return dict(tool=tool['id'],status=status,allow=allow,**kwargs)


def check(config,tool,force=False,host=None,session=None,clock=None,spawn=True,background=False):
    now=time.time() if clock is None else clock
    policy=config['policy']
    if not policy['enabled']: return result(tool,'disabled',True)
    key=fingerprint(tool)
    interval=tool.get('interval_seconds',policy['interval_seconds'])
    try:
        validate_bindings(tool)
        current=observed_installation(policy,tool,now,interval,force)
    except Exception as error: return result(tool,'probe_failed',False,error=str(error))
    lease=None
    with state.transaction(policy) as db:
        row=state.row(db,tool['id'],key)
        fresh=row['checked_at'] is not None and 0<=now-row['checked_at']<interval and row['metadata']
        if force or not fresh:
            if row['lease'] and row['lease_until']>now:
                return result(tool,'check_or_download_pending',False,retry_after=row['lease_until'])
            if row['retry_after']>now and not force:
                return result(tool,'check_failed',False,error=row['error'] or row['stage_error'],retry_after=row['retry_after'])
            lease=secrets.token_hex(16)
            db.execute('UPDATE tools SET lease=?,lease_until=? WHERE id=?',(lease,now+60,tool['id']))
    if lease and background:
        try:
            start_worker(config,tool,lease,'check')
            return result(tool,'check_pending',False,next_step='One owned hidden metadata check is running. Retry this tool after completion; do not bypass the gate or repeatedly start checks.')
        except Exception as error:
            timeout_worker(config,tool,lease,str(error))
            return result(tool,'check_failed',False,error=str(error))
    if lease:
        try:
            metadata=providers.metadata(tool)
            providers.version(metadata['latest'])
            encoded=json.dumps(metadata)
            with state.transaction(policy) as db:
                live=db.execute('SELECT * FROM tools WHERE id=?',(tool['id'],)).fetchone()
                if not live or live['fingerprint']!=key or live['lease']!=lease: return result(tool,'policy_or_lease_changed',False)
                db.execute('UPDATE tools SET checked_at=?,metadata=?,error=NULL,retry_after=0,lease=NULL,lease_until=0 WHERE id=?',(now,encoded,tool['id']))
            row['checked_at'],row['metadata']=now,encoded
        except Exception as error:
            with state.transaction(policy) as db:
                db.execute('UPDATE tools SET error=?,retry_after=?,lease=NULL,lease_until=0 WHERE id=? AND lease=?',(str(error),now+policy['retry_seconds'],tool['id'],lease))
            return result(tool,'check_failed',False,error=str(error),retry_after=now+policy['retry_seconds'])
    metadata=json.loads(row['metadata'])
    have,want=providers.version(current['version']),providers.version(metadata['latest'])
    common={'current':current['version'],'latest':metadata['latest'],'source_url':metadata['source_url'],
            'checked_at':row['checked_at'],'next_check_at':row['checked_at']+interval,'network_checked':lease is not None,
            'runtime_assurance':'observed local version and configured binding; host-loaded MCP process identity is not independently attested'}
    if host and session:
        current_binding=binding(tool)
        with state.transaction(policy) as db:
            old=db.execute('SELECT binding FROM sessions WHERE host=? AND session=? AND tool=?',(host,session,tool['id'])).fetchone()
            if old and old['binding']!=current_binding:
                return result(tool,'reconnect_required',False,**common,next_step='Tool binding changed during this session. Reconnect the MCP client; do not treat a downloaded package as an active update.')
            db.execute('INSERT OR IGNORE INTO sessions VALUES (?,?,?,?)',(host,session,tool['id'],current_binding))
    if have>=want: return result(tool,'current' if have==want else 'ahead_of_stable',True,**common)
    pin=tool.get('pin')
    if pin:
        if pin['version']==current['version'] and pin['latest']==metadata['latest']:
            return result(tool,'migration_exception',True,**common,reason=pin['reason'],evidence_url=pin['evidence_url'],
                          next_step='Resolve this recorded migration before activating the available update. This exception expires when the observed latest version changes.')
        return result(tool,'migration_exception_stale',False,**common,next_step='The migration exception no longer matches installed/latest versions; review it again.')
    conflict=providers.compatibility(tool,current,metadata)
    if conflict:
        return result(tool,'migration_review_required',False,**common,reason=conflict,
                      next_step='Inspect upstream migration notes and actual runtime/add-on requirements. Resolve and verify compatibility, or record a latest-version-bound exception with evidence; never force an unsafe replacement.')
    cached=state.read(policy,tool['id'])
    if cached.get('artifact') and cached['artifact']['version']==metadata['latest']:
        return result(tool,'activation_required',False,**common,artifact=cached['artifact'],
            next_step='Verified update downloaded side-by-side. Install/test it through the tool manager, update the canonical binding and reconnect MCP if applicable. Preserve active work and old versions; verify the effective version before retrying.')
    if cached.get('lease') and cached['lease_until']>now:
        return result(tool,'download_pending',False,**common)
    if cached.get('retry_after',0)>now:
        return result(tool,'download_failed',False,**common,error=cached.get('stage_error'),retry_after=cached['retry_after'])
    if not policy['auto_download'] or not spawn:
        return result(tool,'download_required',False,**common,next_step='Run the exact installed updater check command to start verified staging; use is blocked until the update is installed and observed.')
    token=secrets.token_hex(16)
    with state.transaction(policy) as db:
        live=db.execute('SELECT * FROM tools WHERE id=?',(tool['id'],)).fetchone()
        if not live or live['fingerprint']!=key: return result(tool,'policy_or_lease_changed',False)
        if live['lease'] and live['lease_until']>now: return result(tool,'download_pending',False,**common)
        db.execute('UPDATE tools SET lease=?,lease_until=?,stage_error=NULL WHERE id=?',(token,now+360,tool['id']))
    try:
        start_worker(config,tool,token,'download')
    except Exception as error:
        with state.transaction(policy) as db:
            db.execute('UPDATE tools SET stage_error=?,lease=NULL,lease_until=0,retry_after=? WHERE id=? AND lease=?',(str(error),now+policy['retry_seconds'],tool['id'],token))
        return result(tool,'download_failed',False,**common,error=str(error))
    return result(tool,'download_pending',False,**common,next_step='An owned hidden worker is downloading and verifying the update. Retry after staging; do not rerun downloads or use the stale tool.')


def start_worker(config,tool,token,operation):
    root=Path(config['policy']['state_dir'])/'jobs'
    root.mkdir(parents=True,exist_ok=True)
    args=[sys.executable,'-I',str(Path(__file__).resolve().parent/'updater.py'),'--config',config['_path'],
          'worker','--tool',tool['id'],'--lease',token,'--operation',operation]
    options={'stdin':subprocess.DEVNULL,'close_fds':True}
    if os.name=='nt': options['creationflags']=subprocess.CREATE_NO_WINDOW | subprocess.DETACHED_PROCESS
    else: options['start_new_session']=True
    with (root/(token+'.log')).open('xb') as log:
        subprocess.Popen(args,stdout=log,stderr=log,**options)  # nosec B603


def timeout_worker(config,tool,token,message='Owned update worker exceeded its absolute deadline'):
    with state.transaction(config['policy']) as db:
        db.execute('UPDATE tools SET error=?,retry_after=?,lease=NULL,lease_until=0 WHERE id=? AND lease=?',
            (message,time.time()+config['policy']['retry_seconds'],tool['id'],token))


def worker(config,tool,token,operation='download'):
    from staging import download
    policy=config['policy']; key=fingerprint(tool)
    with state.transaction(policy) as db:
        existing=db.execute('SELECT * FROM tools WHERE id=?',(tool['id'],)).fetchone()
        if not policy['enabled'] or not existing or existing['fingerprint']!=key or existing['lease']!=token or existing['lease_until']<=time.time():
            raise ValueError('Download lease is no longer owned')
        metadata=json.loads(existing['metadata']) if existing['metadata'] else None
    try:
        if operation=='check':
            metadata=providers.metadata(tool)
            providers.version(metadata['latest'])
            with state.transaction(policy) as db:
                changed=db.execute('UPDATE tools SET checked_at=?,metadata=?,error=NULL,retry_after=0,lease=NULL,lease_until=0 WHERE id=? AND fingerprint=? AND lease=?',
                    (time.time(),json.dumps(metadata),tool['id'],key,token))
                if changed.rowcount!=1: raise ValueError('Metadata completed after policy/lease changed')
            return
        if not metadata: raise ValueError('Download requires successful metadata evidence')
        receipt=download(policy,tool,metadata,token)
        with state.transaction(policy) as db:
            written=db.execute('UPDATE tools SET artifact=?,lease=NULL,lease_until=0,stage_error=NULL,retry_after=0 WHERE id=? AND fingerprint=? AND lease=?',
                       (json.dumps(receipt),tool['id'],key,token))
            if written.rowcount!=1: raise ValueError('Download completed after its policy/lease changed; artifact preserved but not activated')
    except Exception as error:
        with state.transaction(policy) as db:
            column='error' if operation=='check' else 'stage_error'
            # Static statements keep source-controlled SQL independent of metadata.
            statement=('UPDATE tools SET error=?,lease=NULL,lease_until=0,retry_after=? WHERE id=? AND fingerprint=? AND lease=?'
                if column=='error' else 'UPDATE tools SET stage_error=?,lease=NULL,lease_until=0,retry_after=? WHERE id=? AND fingerprint=? AND lease=?')
            db.execute(statement,(str(error),time.time()+policy['retry_seconds'],tool['id'],key,token))
        raise

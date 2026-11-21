"""Cheap cached PreToolUse checks. Downloads run in a separate owned hidden job."""
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import sys
import time

from config import load
import engine
import state


def command_tokens(command,windows=None):
    windows=os.name=='nt' if windows is None else windows
    tokens=shlex.split(command,posix=not windows)
    if windows:
        tokens=[s[1:-1] if len(s)>=2 and s[0]==s[-1] and s[0] in ('"',"'") else s for s in tokens]
    return tokens[1:] if tokens[:1]==['&'] else tokens


def program_name(value):
    path=Path(value)
    return (path.stem if path.suffix.lower() in ('.exe','.cmd','.bat') else path.name).lower()


def repair_call(tool,args,config):
    if not isinstance(args,dict): return False
    if tool not in ('cli_run','mcp__kritic__cli_run','exec_command','Bash','run_command','functions.exec_command'):
        return False
    name=tool.rsplit('__',1)[-1].rsplit('.',1)[-1]
    if name in ('cli_run',):
        if not isinstance(args.get('program',''),str) or not isinstance(args.get('args',[]),list) or any(not isinstance(s,str) for s in args.get('args',[])):
            return False
        tokens=[args.get('program',''),*args.get('args',[])]
    elif name in ('exec_command','Bash','run_command'):
        try: tokens=command_tokens(args.get('cmd') or args.get('command') or '')
        except (ValueError,TypeError): return False
    else: return False
    if len(tokens)<4 or Path(tokens[0]).name.lower() not in ('python','python.exe','python3') or tokens[1]!='-I': return False
    interpreter=Path(shutil.which(tokens[0]) or tokens[0]).resolve()
    trusted={Path(sys.executable).resolve()}
    trusted.update(Path(p).resolve() for alias in ('python','python3') if (p:=shutil.which(alias)))
    if interpreter not in trusted: return False
    if Path(tokens[2]).resolve()!=Path(__file__).resolve().parent/'updater.py': return False
    tail=tokens[3:]
    if tail[:1]==['--config']:
        if len(tail)<3 or Path(tail[1]).resolve()!=Path(config['_path']).resolve(): return False
        tail=tail[2:]
    if not tail or tail[0] not in ('schema','status','check','inventory'): return False
    # Bootstrap is an exact argv grammar; no shell chaining or worker impersonation.
    rest=tail[1:]
    if rest==[] or rest==['--force']: return True
    return len(rest) in (2,3) and rest[0]=='--tool' and any(t['id']==rest[1] for t in config['tool']) and (len(rest)==2 or rest[2]=='--force')


def matched(config,tool,args):
    name=tool.rsplit('__',1)[-1].rsplit('.',1)[-1]
    program=None; parts=[]
    if name=='cli_run':
        program=args.get('program'); parts=[program,*args.get('args',[])]
    elif name in ('exec_command','Bash','run_command'):
        try:
            parts=command_tokens(args.get('cmd') or args.get('command') or '')
            if parts: program=parts[0]
        except ValueError: pass
    selected=[]
    for entry in config['tool']:
        if any(tool.startswith(prefix) for prefix in entry['tool_prefixes']): selected.append(entry); continue
        if program and program_name(program) in {program_name(p) for p in entry['programs']}:
            selected.append(entry)
        elif program and len(parts)>1 and entry['installed']['kind']=='npm':
            script=Path(parts[1])
            if script.name.lower() in {Path(p).name.lower() for p in entry['programs']} or (script.is_absolute() and script.resolve().is_relative_to(Path(entry['installed']['path']).parent)):
                selected.append(entry)
    return selected


def invocation_conflict(tool,args,entry):
    name=tool.rsplit('__',1)[-1].rsplit('.',1)[-1]
    if name=='cli_run': parts=[args.get('program',''),*args.get('args',[])]
    elif name in ('exec_command','Bash','run_command'):
        try: parts=command_tokens(args.get('cmd') or args.get('command') or '')
        except ValueError: return 'Cannot verify the requested shell executable; use an explicit canonical argv'
        if parts[:1]==['&']: parts=parts[1:]
    else: return None
    if not parts: return 'Missing executable identity'
    actual=Path(shutil.which(parts[0]) or parts[0]).resolve()
    probe=entry['installed']
    if probe['kind']=='command':
        if actual!=Path(probe['argv'][0]).resolve(): return 'Requested executable differs from the current canonical installation: '+probe['argv'][0]
    elif probe['kind']=='npm':
        root=Path(probe['path']).parent
        if len(parts)<2 or not Path(parts[1]).is_absolute() or Path(parts[1]).resolve() not in engine.canonical_scripts(entry):
            return 'Use the canonical Node runtime and script below '+str(root)+'; an alias or old script cannot attest the current version'
        if probe.get('runtime') and actual!=Path(probe['runtime']).resolve(): return 'Requested Node executable differs from the configured compatible runtime'
    elif probe.get('runtime') and actual.parent!=Path(probe['runtime']).parent:
        return 'Requested executable is outside the canonical Python environment: '+str(Path(probe['runtime']).parent)
    return None


def control_plane(tool,args):
    if tool in ('search_tools','tool_search','discover_tools','list_tools','Read','Glob','Grep','read_file','list_directory',
                'tool_search.search','functions.search','collaboration.send_message','mcp__codex_app__send_message_to_thread'):
        return True
    return tool in ('mcp__kev_tools__worktree_evaluate','mcp__kev-tools__worktree_evaluate') and args.get('action') in ('schema','status','declare','release','transfer','recover')


def response(host,message,deny=False):
    if not message: return {}
    if host=='hermes': return {'action':'block','message':message} if deny else {'context':message}
    output={'hookEventName':'PreToolUse'}
    if deny: output.update(permissionDecision='deny',permissionDecisionReason=message)
    else: output['additionalContext']=message
    return {'hookSpecificOutput':output}


def evaluate(config,host,event,payload):
    if host not in ('codex','claude','hermes'): raise ValueError('Unsupported update hook host')
    if not config['policy']['enabled']: return {}
    session=payload.get('session_id') or payload.get('sessionId') or payload.get('task_id')
    if not isinstance(session,str) or not session: raise ValueError('Actual session identity is required for update notices/bindings')
    if event=='session':
        engine.session_start(config,host,session)
        gaps=[g for g in config.get('_coverage',[]) if g['status'] in ('local_unmanaged','inventory_failed')]
        if gaps:
            message='Kritic update inventory: '+str(len(config['tool']))+' managed installations; '+str(len(gaps))+' local/provider gaps. Use the matching updater inventory command; unknown local tools are not certified fresh. Repair their source/binding rather than assuming latest.'
            if host=='hermes': return {'context':message}
            return {'hookSpecificOutput':{'hookEventName':'SessionStart','additionalContext':message}}
        return {}
    tool=payload.get('tool_name','')
    args=payload.get('tool_input') or payload.get('args') or {}
    if not isinstance(args,dict): raise ValueError('Tool arguments must be an object')
    if control_plane(tool,args): return {}
    if repair_call(tool,args,config): return {}
    for gap in config.get('_coverage',[]):
        if gap['status']=='local_unmanaged' and any(tool.startswith(prefix) for prefix in gap['tool_prefixes']):
            return response(host,'Kritic tool-update provider missing: '+json.dumps(gap)+'. Identify its official update source and enroll a verified binding, or record a grounded development/host-managed exclusion. Do not label unknown tools current.',True)
    notices=[]; started=time.monotonic()
    for entry in matched(config,tool,args):
        if any(tool.startswith(prefix) for prefix in entry['tool_prefixes']) and entry['installed'].get('role','tool')!='application' and not entry['installed'].get('client_bindings'):
            return response(host,'Kritic tool-update binding is unverified: enroll the actual client registration for '+entry['id']+'. A newer package elsewhere cannot certify this MCP server.',True)
        if time.monotonic()-started>16:
            return response(host,'Tool-update check budget reached; retry to finish remaining managed checks. Earlier checks are cached; no unchecked tool use was allowed.',True)
        report=engine.check(config,entry,host=host,session=session,background=True)
        if not report['allow']:
            command='python -I '+shlex.quote(str(Path(__file__).resolve().parent/'updater.py'))+' --config '+shlex.quote(config['_path'])+' status --tool '+entry['id']
            return response(host,'Kritic tool-update gate: '+json.dumps(report,ensure_ascii=True)+
                '. Repair/check status with the matching updater: '+command+
                '. This is an operational freshness requirement, not model approval. Preserve existing work and old versions.',True)
        mismatch=invocation_conflict(tool,args,entry)
        if mismatch: return response(host,'Kritic tool-update gate: stale/unverified executable binding. '+mismatch,True)
        if report['status']=='migration_exception':
            # Time/network_checked fields are excluded so a fresh cache read is silent.
            signature=hashlib.sha256(json.dumps({k:report[k] for k in ('tool','status','current','latest','reason')},sort_keys=True).encode()).hexdigest()
            with state.transaction(config['policy']) as db:
                previous=db.execute('SELECT signature FROM notices WHERE host=? AND session=? AND tool=?',(host,session,entry['id'])).fetchone()
                if not previous or previous[0]!=signature:
                    notices.append(json.dumps(report,ensure_ascii=True))
                    db.execute('INSERT INTO notices VALUES (?,?,?,?) ON CONFLICT(host,session,tool) DO UPDATE SET signature=excluded.signature',(host,session,entry['id'],signature))
    return response(host,'Kritic tool-update notice: '+'; '.join(notices)) if notices else {}

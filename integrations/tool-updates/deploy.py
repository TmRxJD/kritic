"""Additive managed-hook upgrade; never replaces worktree-recovery hooks."""
import argparse
import csv
import ctypes
import hashlib
import json
import io
import os
from pathlib import Path
import shutil
import sys
import subprocess  # nosec B404
import time
import re

sys.path.insert(0,str(Path(__file__).resolve().parent))

FILES=('config.py','state.py','providers.py','staging.py','engine.py','hook.py','updater.py','inventory.py','hermes_plugin.py','README.md')


def copy_dacl(source,target):
    api=ctypes.windll.advapi32
    get=api.GetFileSecurityW
    get.argtypes=[ctypes.c_wchar_p,ctypes.c_uint32,ctypes.c_void_p,ctypes.c_uint32,ctypes.POINTER(ctypes.c_uint32)]
    get.restype=ctypes.c_int
    needed=ctypes.c_uint32()
    get(str(source),4,None,0,ctypes.byref(needed))
    if not needed.value: raise OSError('Cannot inspect existing config DACL')
    descriptor=ctypes.create_string_buffer(needed.value)
    if not get(str(source),4,descriptor,needed.value,ctypes.byref(needed)): raise OSError('Cannot read existing config DACL')
    control=ctypes.c_uint16(); revision=ctypes.c_uint32()
    inspect=api.GetSecurityDescriptorControl
    inspect.argtypes=[ctypes.c_void_p,ctypes.POINTER(ctypes.c_uint16),ctypes.POINTER(ctypes.c_uint32)]
    inspect.restype=ctypes.c_int
    if not inspect(descriptor,ctypes.byref(control),ctypes.byref(revision)): raise OSError('Cannot inspect DACL protection')
    put=api.SetFileSecurityW
    put.argtypes=[ctypes.c_wchar_p,ctypes.c_uint32,ctypes.c_void_p]; put.restype=ctypes.c_int
    security=4|(0x80000000 if control.value&0x1000 else 0x20000000)
    if not put(str(target),security,descriptor): raise OSError('Cannot preserve config DACL')


def protect_directory(path):
    path.mkdir(parents=True,exist_ok=False,mode=0o700)
    if os.name=='nt':
        system=Path(os.environ.get('SystemRoot','C:/Windows'))/'System32'
        identity=subprocess.run([str(system/'whoami.exe'),'/user','/fo','csv','/nh'],capture_output=True,text=True,check=True,
            creationflags=subprocess.CREATE_NO_WINDOW,timeout=10)  # nosec B603
        sid=next(csv.reader(io.StringIO(identity.stdout)))[1]
        if not re.fullmatch(r'S-\d+(?:-\d+)+',sid): raise ValueError('Cannot resolve current user SID')
        subprocess.run([str(system/'icacls.exe'),str(path),'/inheritance:r','/grant:r','*'+sid+':(OI)(CI)F','*S-1-5-18:(OI)(CI)F'],
            capture_output=True,check=True,creationflags=subprocess.CREATE_NO_WINDOW,timeout=10)  # nosec B603


def write_private(path,data,acl_source=None):
    descriptor=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
    with os.fdopen(descriptor,'wb') as stream:
        if acl_source and os.name=='nt':
            copy_dacl(acl_source,path)
        elif acl_source:
            os.chmod(path,acl_source.stat().st_mode&0o777)
        stream.write(data)


def install(runtime,config,codex=None,claude=None,hermes=None,replace_runtime=None):
    from config import load
    policy=load(config)
    runtime=Path(runtime).resolve(); source=Path(__file__).resolve().parent
    runtime.mkdir(parents=True,exist_ok=True)
    for name in FILES:
        old=source/name; new=runtime/name
        if new.exists() and new.read_bytes()!=old.read_bytes(): raise ValueError('Preserve immutable previous update runtime: '+str(new))
        if not new.exists(): shutil.copy2(old,new)
    plans=[]
    for host,home,file in [('codex',codex,'hooks.json'),('claude',claude,'settings.json')]:
        if home is None: continue
        path=Path(home)/file
        before=path.read_bytes() if path.exists() else None
        value=json.loads(before) if before else {}
        hooks=value.setdefault('hooks',{})
        for event,action in [('SessionStart','session'),('PreToolUse','pretool')]:
            command=f'python -I "{(runtime/"updater.py").as_posix()}" --config "{Path(config).resolve().as_posix()}" hook {host} {action}'
            groups=hooks.setdefault(event,[])
            if replace_runtime:
                old=f'python -I "{(Path(replace_runtime).resolve()/"updater.py").as_posix()}" --config "{Path(config).resolve().as_posix()}" hook {host} {action}'
                kept=[]
                for group in groups:
                    entries=group.get('hooks',[])
                    remaining=[item for item in entries if item.get('command')!=old]
                    if len(remaining)!=len(entries):
                        if remaining: kept.append(dict(group,hooks=remaining))
                    else: kept.append(group)
                hooks[event]=groups=kept
            if not any(item.get('command')==command for group in groups for item in group.get('hooks',[])):
                groups.append({'hooks':[{'type':'command','command':command,'timeout':60}]})
        plans.append((path,before,(json.dumps(value,indent=2)+'\n').encode()))
    if hermes:
        home=Path(hermes); plugin=home/'plugins/kritic-tool-updates'
        if plugin.exists():
            marker=plugin/'managed-by-kritic.json'
            if not marker.exists() or json.loads(marker.read_text()).get('integration')!='tool-updates':
                raise ValueError('Preserve unrelated Hermes plugin')
        body=(runtime/'hermes_plugin.py').read_bytes()
        plans.extend([(plugin/'__init__.py',(plugin/'__init__.py').read_bytes() if (plugin/'__init__.py').exists() else None,body),
            (plugin/'updater.json',(plugin/'updater.json').read_bytes() if (plugin/'updater.json').exists() else None,
             json.dumps({'updater':str(runtime/'updater.py'),'config':str(Path(config).resolve()),'python':sys.executable}).encode()),
            (plugin/'plugin.yaml',(plugin/'plugin.yaml').read_bytes() if (plugin/'plugin.yaml').exists() else None,
             b'name: kritic-tool-updates\nversion: 0.1.0\ndescription: Cached operational tool-update gate\nhooks:\n  - pre_tool_call\n'),
            (plugin/'managed-by-kritic.json',(plugin/'managed-by-kritic.json').read_bytes() if (plugin/'managed-by-kritic.json').exists() else None,
             b'{"integration":"tool-updates"}\n')])
        import yaml
        path=home/'config.yaml'; before=path.read_bytes(); value=yaml.safe_load(before)
        # Follow the host's actual plugins.enrolled configuration rather than
        # inventing an enabled flag; deployment checks its known schema.
        plugins=value.setdefault('plugins',{})
        if not isinstance(plugins,dict): raise ValueError('Unsupported Hermes plugins config')
        enabled=plugins.setdefault('enabled',[])
        if not isinstance(enabled,list): raise ValueError('Unsupported Hermes plugin enablement schema')
        if 'kritic-tool-updates' not in enabled: enabled.append('kritic-tool-updates')
        plans.append((path,before,yaml.safe_dump(value,sort_keys=False).encode()))
    backup=Path(policy['policy']['state_dir'])/('enrollment-backup-'+str(time.time_ns()))
    backup.parent.mkdir(parents=True,exist_ok=True)
    protect_directory(backup)
    written=[]
    try:
        for index,(path,before,after) in enumerate(plans):
            if (path.read_bytes() if path.exists() else None)!=before: raise ValueError('Concurrent settings change: '+str(path))
            if before is not None: write_private(backup/str(index),before)
            path.parent.mkdir(parents=True,exist_ok=True)
            pending=path.with_name(path.name+'.kritic-updates.pending')
            if pending.exists(): raise ValueError('Preserve existing pending config')
            write_private(pending,after,path if before is not None else None)
            if (path.read_bytes() if path.exists() else None)!=before: raise ValueError('Concurrent settings change before replace')
            pending.replace(path); written.append((path,before,after))
    except BaseException:
        for path,before,after in reversed(written):
            if path.read_bytes()!=after: raise ValueError('Settings changed during rollback; preserve current file: '+str(path))
            if before is None: path.unlink()
            else:
                pending=path.with_name(path.name+'.kritic-updates.rollback')
                if pending.exists(): raise ValueError('Preserve previous rollback file')
                write_private(pending,before,path); pending.replace(path)
        raise
    return {'runtime':str(runtime),'config':str(Path(config).resolve()),'updated_files':[str(p) for p,_,_ in plans],
            'private_backup':str(backup),
            'source_hashes':{name:hashlib.sha256((runtime/name).read_bytes()).hexdigest() for name in FILES}}


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime',required=True); parser.add_argument('--config',required=True)
    parser.add_argument('--codex-home'); parser.add_argument('--claude-home'); parser.add_argument('--hermes-home')
    parser.add_argument('--replace-runtime')
    args=parser.parse_args()
    print(json.dumps(install(args.runtime,args.config,args.codex_home,args.claude_home,args.hermes_home,args.replace_runtime)))

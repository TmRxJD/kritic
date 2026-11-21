"""Strict operational update policy; separate from older Kritic critic configs."""
import hashlib
import json
import os
from pathlib import Path
import re
import tomllib

DEFAULT_INTERVAL = 7 * 24 * 60 * 60


def fields(value, allowed, required=()):
    if not isinstance(value,dict) or set(value)-set(allowed) or set(required)-set(value):
        raise ValueError('Invalid update-policy fields; expected '+', '.join(allowed))


def absolute(value):
    if not isinstance(value,str) or not value or not Path(value).is_absolute():
        raise ValueError('Update policy requires absolute local paths')
    return str(Path(value).resolve())


def load(path=None):
    explicit=path is not None or bool(os.environ.get('KRITIC_UPDATES_CONFIG'))
    path=Path(path or os.environ.get('KRITIC_UPDATES_CONFIG',Path.home()/'.kritic/updates.toml'))
    if not path.exists():
        if explicit: raise ValueError('Enrolled update-policy file is missing: '+str(path))
        return {'version':1,'policy':{'enabled':False,'interval_seconds':DEFAULT_INTERVAL},'tool':[], '_path':str(path)}
    if path.stat().st_size>256_000:
        raise ValueError('Update policy exceeds256 KiB')
    value=tomllib.loads(path.read_text(encoding='utf8'))
    fields(value,('version','policy','tool'),('version',))
    if value['version']!=1 or isinstance(value['version'],bool):
        raise ValueError('Update policy version must be1')
    policy=value.setdefault('policy',{})
    fields(policy,('enabled','interval_seconds','retry_seconds','state_dir','auto_download','max_download_bytes','auto_discover','client_configs','exclude'))
    policy.setdefault('enabled',True)
    policy.setdefault('interval_seconds',DEFAULT_INTERVAL)
    policy.setdefault('retry_seconds',600)
    policy.setdefault('auto_download',True)
    policy.setdefault('auto_discover',True)
    policy.setdefault('client_configs',[])
    policy.setdefault('exclude',[])
    policy.setdefault('max_download_bytes',1024**3)
    for flag in ('enabled','auto_download','auto_discover'):
        if not isinstance(policy[flag],bool): raise ValueError(flag+' must be boolean')
    for name,lo,hi in [('interval_seconds',300,31*86400),('retry_seconds',10,86400),('max_download_bytes',1024,4*1024**3)]:
        item=policy[name]
        if not isinstance(item,int) or isinstance(item,bool) or not lo<=item<=hi:
            raise ValueError(name+' outside supported bounds')
    policy['state_dir']=absolute(policy.get('state_dir',str(Path.home()/'.kritic/tool-updates')))
    if not isinstance(policy['client_configs'],list) or len(policy['client_configs'])>8: raise ValueError('At most8 client config paths are allowed')
    policy['client_configs']=[absolute(p) for p in policy['client_configs']]
    if not isinstance(policy['exclude'],list) or len(policy['exclude'])>128: raise ValueError('Invalid update exclusions')
    for exclusion in policy['exclude']:
        fields(exclusion,('server','reason'),('server','reason'))
        if any(not isinstance(exclusion[k],str) or not exclusion[k].strip() for k in ('server','reason')): raise ValueError('Update exclusions need exact server and reason')
    tools=value.setdefault('tool',[])
    if not isinstance(tools,list) or len(tools)>128: raise ValueError('Expected at most128 managed tools')
    coverage=[]
    if policy['auto_discover'] and policy['client_configs']:
        from inventory import discover
        discovered,coverage=discover(policy['client_configs'],tools,policy['exclude'])
        tools.extend(discovered)
        if len(tools)>128: raise ValueError('Discovered tool inventory exceeds128; narrow the configured corpus')
    seen=set()
    for tool in tools:
        fields(tool,('id','tool_prefixes','programs','source','installed','compatibility','interval_seconds','pin'),('id','source','installed'))
        identifier=tool['id']
        if not isinstance(identifier,str) or not re.fullmatch(r'[a-z0-9][a-z0-9_-]{0,63}',identifier) or identifier in seen:
            raise ValueError('Tool IDs must be unique safe identifiers')
        seen.add(identifier)
        for name in ('tool_prefixes','programs'):
            values=tool.setdefault(name,[])
            if not isinstance(values,list) or len(values)>64 or any(not isinstance(s,str) or not s or len(s)>512 for s in values):
                raise ValueError('Invalid tool matchers')
        source=tool['source']
        fields(source,('kind','package','repository','asset','notes_path','notes_ref'),('kind',))
        if source['kind'] not in ('npm','pypi','github','blender'): raise ValueError('Unsupported update source')
        allowed={'npm':('kind','package','repository','notes_path','notes_ref'),
                 'pypi':('kind','package','repository','notes_path','notes_ref'),
                 'github':('kind','repository','asset'),'blender':('kind',)}[source['kind']]
        required={'npm':('kind','package'),'pypi':('kind','package'),'github':('kind','repository','asset'),'blender':('kind',)}[source['kind']]
        fields(source,allowed,required)
        installed=tool['installed']
        fields(installed,('kind','path','argv','runtime','client_bindings','server','entrypoint','role'),('kind',))
        installed.setdefault('role','tool')
        if installed['role'] not in ('tool','application'): raise ValueError('Invalid installation role')
        if installed['kind'] in ('npm','python_metadata'):
            installed['path']=absolute(installed.get('path'))
        elif installed['kind']=='command':
            args=installed.get('argv')
            if not isinstance(args,list) or not 1<=len(args)<=16 or any(not isinstance(s,str) or len(s)>4096 for s in args):
                raise ValueError('A trusted absolute version-probe argv is required')
            args[0]=absolute(args[0])
        else: raise ValueError('Unsupported installed-version probe')
        if 'runtime' in installed: installed['runtime']=absolute(installed['runtime'])
        bindings=installed.setdefault('client_bindings',[])
        if not isinstance(bindings,list) or len(bindings)>8: raise ValueError('Invalid binding inventory')
        installed['client_bindings']=[absolute(p) for p in bindings]
        tool.setdefault('compatibility','same_major')
        if tool['compatibility'] not in ('same_major','same_minor','manual'): raise ValueError('Invalid compatibility policy')
        if 'interval_seconds' in tool and (type(tool['interval_seconds']) is not int or not 300<=tool['interval_seconds']<=31*86400):
            raise ValueError('Invalid per-tool interval')
        if 'pin' in tool:
            fields(tool['pin'],('version','latest','reason','evidence_url'),('version','latest','reason','evidence_url'))
            if any(not isinstance(tool['pin'][k],str) or not tool['pin'][k].strip() or len(tool['pin'][k])>2000 for k in ('version','latest','reason','evidence_url')):
                raise ValueError('A migration exception needs current/latest versions, reason and evidence URL')
    value['_path']=str(path.resolve())
    value['_coverage']=coverage
    return value


def fingerprint(tool):
    return hashlib.sha256(json.dumps(tool,sort_keys=True,separators=(',',':')).encode()).hexdigest()

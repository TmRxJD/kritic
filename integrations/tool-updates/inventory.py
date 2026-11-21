"""Bounded registration discovery. No workspace indexing or secret-bearing output."""
import configparser
import email.parser
import hashlib
import json
from pathlib import Path
import re
import shutil
import tomllib


def client_servers(path):
    path=Path(path)
    if not path.is_file() or path.stat().st_size>2*1024**2: raise ValueError('Client configuration missing/oversized')
    content=path.read_text(encoding='utf8')
    if path.suffix=='.toml': data=tomllib.loads(content)
    elif path.suffix=='.json': data=json.loads(content)
    else:
        import yaml
        data=yaml.safe_load(content)
    return data.get('mcp_servers',data.get('mcpServers',{}))


def prefixes(name):
    names={name,re.sub(r'[^A-Za-z0-9_]','_',name)}
    return sorted({'mcp__'+value+'__' for value in names})


def npm_script(args):
    if not args or not isinstance(args[0],str): return None
    script=Path(args[0])
    if not script.is_absolute() or not script.is_file(): return None
    for parent in list(script.parents)[:8]:
        package=parent/'package.json'
        if not package.is_file() or package.stat().st_size>1024**2: continue
        data=json.loads(package.read_text(encoding='utf8'))
        if data.get('private') or not data.get('name') or not data.get('version'): return None
        if 'node_modules' not in {part.lower() for part in package.parts}: return None
        return package,data,script.name
    return None


def python_distribution(executable):
    root=executable.parent.parent
    locations=[root/'Lib/site-packages',*list((root/'lib').glob('python*/site-packages'))]
    for location in locations[:8]:
        candidates=list(location.glob('*.dist-info'))
        if len(candidates)>512: raise ValueError('Python tool distribution inventory exceeds512 entries')
        for candidate in candidates:
            direct=candidate/'direct_url.json'
            if direct.is_file():
                provenance=json.loads(direct.read_text(encoding='utf8'))
                if provenance.get('dir_info') or provenance.get('vcs_info') or provenance.get('url','').startswith('file:'): continue
            entry=candidate/'entry_points.txt'; metadata=candidate/'METADATA'
            if not entry.is_file() or not metadata.is_file() or entry.stat().st_size>65536 or metadata.stat().st_size>1024**2: continue
            parser=configparser.ConfigParser(interpolation=None)
            parser.read_string(entry.read_text(encoding='utf8'))
            if parser.has_section('console_scripts') and executable.stem in parser['console_scripts']:
                info=email.parser.Parser().parsestr(metadata.read_text(encoding='utf8'))
                runtime=executable.parent/('python.exe' if executable.suffix=='.exe' else 'python')
                if runtime.is_file(): return metadata,info['Name'],runtime
    return None


def discover(paths,explicit,exclusions):
    tools=[]; coverage=[]
    covered={tool['installed'].get('server',tool['id']) for tool in explicit}
    excluded={item['server']:item['reason'] for item in exclusions}
    for path in paths:
        try: servers=client_servers(path)
        except Exception as error:
            coverage.append({'client':path,'status':'inventory_failed','reason':str(error),'tool_prefixes':[]}); continue
        for name,server in servers.items():
            if not isinstance(server,dict) or server.get('enabled') is False or server.get('disabled') is True: continue
            if name in covered: continue
            if name in excluded:
                coverage.append({'server':name,'client':path,'status':'explicit_exclusion','reason':excluded[name],'tool_prefixes':prefixes(name)}); continue
            command=server.get('command')
            reason='No canonical installed-package/version provider was identified; enroll an official source before use'
            if not command:
                coverage.append({'server':name,'client':path,'status':'host_managed','reason':'Remote/host-managed service has no local download authority','tool_prefixes':prefixes(name)}); continue
            executable=Path(shutil.which(command) or command).resolve()
            installed=None; source=None; programs=[]
            try:
                package=npm_script(server.get('args',[]))
                if package and executable.stem.lower() in ('node','nodejs'):
                    manifest,data,script=package
                    installed={'kind':'npm','path':str(manifest),'runtime':str(executable),'server':name,'client_bindings':[path]}
                    source={'kind':'npm','package':data['name']}; programs=[script,*list(data.get('bin',{}))] if isinstance(data.get('bin',{}),dict) else [script]
                elif executable.is_file():
                    package=python_distribution(executable)
                    if package:
                        metadata,package_name,runtime=package
                        installed={'kind':'python_metadata','path':str(metadata),'runtime':str(runtime),'server':name,'client_bindings':[path]}
                        source={'kind':'pypi','package':package_name}; programs=[executable.name]
            except Exception as error: reason=str(error)
            if not installed:
                coverage.append({'server':name,'client':path,'status':'local_unmanaged','reason':reason,'tool_prefixes':prefixes(name)}); continue
            identifier='auto-'+re.sub('[^a-z0-9_-]','-',name.lower())[:40]+'-'+hashlib.sha256(path.encode()).hexdigest()[:8]
            tools.append({'id':identifier,'tool_prefixes':prefixes(name),'programs':programs,'source':source,'installed':installed,'compatibility':'same_major'})
    return tools,coverage

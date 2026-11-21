"""Public official metadata and observed local versions. No model judgments."""
import base64
import email.parser
import hashlib
import json
from pathlib import Path
import re
import subprocess  # nosec B404
import urllib.error
import urllib.parse
import urllib.request

TIMEOUT=8
MAX_METADATA=1024**2


class Redirects(urllib.request.HTTPRedirectHandler):
    def __init__(self,hosts): super().__init__(); self.hosts=set(hosts)
    def redirect_request(self,req,fp,code,msg,headers,newurl):
        validate_url(newurl,self.hosts)
        return super().redirect_request(req,fp,code,msg,headers,newurl)


def validate_url(url,hosts):
    parsed=urllib.parse.urlsplit(url)
    if parsed.scheme!='https' or parsed.hostname not in hosts or parsed.username or parsed.password or parsed.fragment:
        raise ValueError('Update URL is not an approved credential-free HTTPS source')


def fetch(url,hosts,limit=MAX_METADATA):
    validate_url(url,hosts)
    opener=urllib.request.build_opener(Redirects(hosts))
    request=urllib.request.Request(url,headers={'User-Agent':'Kritic-tool-updates/1','Accept':'application/json'})
    with opener.open(request,timeout=TIMEOUT) as response:  # nosec B310
        data=response.read(limit+1)
        if len(data)>limit: raise ValueError('Update metadata exceeds its read limit')
        return data


def version(value):
    if not isinstance(value,str): raise ValueError('Version must be text')
    match=re.fullmatch(r'v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)',value.strip())
    if not match: raise ValueError('A stable numeric major.minor.patch version is required')
    return tuple(map(int,match.groups()))


def satisfies(current,requirement,ecosystem='npm'):
    """Conservative npm/Python comparator subset; unknown syntax requires review."""
    current=version(current)
    if not isinstance(requirement,str) or len(requirement)>512: raise ValueError('Unsupported runtime constraint')
    alternatives=requirement.strip().split('||')
    if not alternatives or len(alternatives)>8: raise ValueError('Unsupported runtime constraint')
    results=[]
    for group in alternatives:
        pattern=r'(\^|~=|~|>=|<=|>|<|==|=)?\s*([0-9]+)(?:\.([0-9]+))?(?:\.([0-9]+))?'
        terms=re.findall(pattern,group)
        remaining=re.sub(pattern,'',group).replace(',','').strip()
        if not terms or remaining: raise ValueError('Runtime constraint syntax needs migration review')
        okay=True
        for op,a,b,c in terms:
            target=(int(a),int(b or 0),int(c or 0))
            partial=not b or not c
            upper=(target[0]+1,0,0) if not b else (target[0],target[1]+1,0)
            if op=='^':
                ceiling=(target[0]+1,0,0) if target[0] or not b else ((0,target[1]+1,0) if target[1] or not c else (0,0,target[2]+1))
                match=target<=current<ceiling
            elif op=='~': match=target<=current<upper
            elif op=='~=':
                if ecosystem!='pypi': raise ValueError('Unsupported runtime constraint')
                match=target<=current<((target[0]+1,0,0) if not c else (target[0],target[1]+1,0))
            elif ecosystem=='npm' and partial and op in ('','=', '==','>','<='):
                match=(target<=current<upper) if op in ('','=','==') else (current>=upper if op=='>' else current<upper)
            else: match={'':current==target,'=':current==target,'==':current==target,'>=':current>=target,'<=':current<=target,'>':current>target,'<':current<target}[op]
            okay=okay and match
        results.append(okay)
    return any(results)


def command(args):
    # Only the local administrator's absolute argv from the policy is executable.
    if not Path(args[0]).is_absolute() or not Path(args[0]).is_file(): raise ValueError('Configured probe executable is missing')
    result=subprocess.run(args,capture_output=True,timeout=8,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0),check=False)  # nosec B603
    if result.returncode: raise ValueError('Installed version probe failed (exit '+str(result.returncode)+')')
    if len(result.stdout)+len(result.stderr)>65536: raise ValueError('Version probe output is oversized')
    text=(result.stdout+result.stderr).decode('utf8',errors='replace')
    match=re.search(r'(?<![\d.])(\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?)(?![\d.])',text)
    if not match: raise ValueError('Version probe did not identify a stable version')
    if re.match(r'\s+(?:alpha|beta|rc|dev)\b',text[match.end():],re.I):
        raise ValueError('Observed prerelease/development version needs an explicit channel policy')
    version(match[1])
    return match[1]


def installed(tool):
    probe=tool['installed']
    kind=probe['kind']
    peers={}
    if kind=='command': current=command(probe['argv'])
    else:
        path=Path(probe['path'])
        if not path.is_file() or path.stat().st_size>MAX_METADATA: raise ValueError('Installed package metadata missing/oversized')
        content=path.read_text(encoding='utf8')
        if kind=='npm':
            data=json.loads(content)
            if data.get('name')!=tool['source'].get('package'): raise ValueError('Installed package identity differs from policy')
            current=data['version']
            peers=data.get('peerDependencies',{})
        else:
            data=email.parser.Parser().parsestr(content)
            if re.sub(r'[-_.]+','-',data['Name']).lower()!=re.sub(r'[-_.]+','-',tool['source'].get('package','')).lower():
                raise ValueError('Installed Python package identity differs from policy')
            current=data['Version']
    version(current)
    runtime=command([probe['runtime'],'--version']) if probe.get('runtime') else None
    return {'version':current,'runtime_version':runtime,'peer_dependencies':peers}


def release_notes(source,latest):
    if not source.get('notes_path'): return {}
    repository=source.get('repository','')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+',repository): raise ValueError('Release-note repository is invalid')
    path=source['notes_path']
    ref=source.get('notes_ref','v{version}').format(version=latest)
    if not re.fullmatch(r'[A-Za-z0-9_./-]+',path) or '..' in path.split('/') or not re.fullmatch(r'[A-Za-z0-9_.-]+',ref):
        raise ValueError('Release-note path/ref is invalid')
    url='https://raw.githubusercontent.com/'+repository+'/'+ref+'/'+path
    text=fetch(url,['raw.githubusercontent.com'],MAX_METADATA).decode('utf8')
    headings=list(re.finditer(r'^##\s+\[?v?(\d+\.\d+\.\d+)\b',text,re.M))
    if not headings: raise ValueError('Release notes have no recognized version sections; migration review required')
    entries=[]
    for i,heading in enumerate(headings[:64]):
        section=text[heading.end():headings[i+1].start() if i+1<len(headings) else len(text)]
        warnings=[line.strip()[:600] for line in section.splitlines() if re.search(r'breaking|migration|no longer|incompatib|requires?\s+(?:ghidra|jdk|node)|evidence\s+(?:format|revision)\s+2',line,re.I)]
        if warnings: entries.append({'version':heading[1],'warnings':warnings[:8]})
    return {'notes_url':url,'notes_review':entries,'notes_oldest_reviewed':headings[min(len(headings),64)-1][1],
            'notes_truncated':len(headings)>64}


def metadata(tool):
    source=tool['source']; kind=source['kind']
    if kind in ('npm','pypi'):
        package=source.get('package','')
        if not re.fullmatch(r'(?:@[A-Za-z0-9_.-]+/)?[A-Za-z0-9_.-]+',package): raise ValueError('Invalid public package identity')
        encoded=urllib.parse.quote(package,safe='')
        url=('https://registry.npmjs.org/'+encoded+'/latest') if kind=='npm' else ('https://pypi.org/pypi/'+encoded+'/json')
        hosts=['registry.npmjs.org'] if kind=='npm' else ['pypi.org','files.pythonhosted.org']
        data=json.loads(fetch(url,hosts))
        if kind=='pypi' and re.sub(r'[-_.]+','-',data['info']['name']).lower()!=re.sub(r'[-_.]+','-',package).lower():
            raise ValueError('PyPI response identity differs from requested package')
        repository=source.get('repository')
        if repository:
            expected=repository.lower()
            if kind=='npm':
                declared=data.get('repository',{})
                urls=[declared.get('url','')] if isinstance(declared,dict) else [declared]
            else: urls=list((data['info'].get('project_urls') or {}).values())
            def matches_repository(value):
                if not isinstance(value,str): return False
                parsed=urllib.parse.urlsplit(value.removeprefix('git+'))
                parts=parsed.path.strip('/').removesuffix('.git').split('/')
                return parsed.hostname=='github.com' and '/'.join(parts[:2]).lower()==expected
            if not any(matches_repository(value) for value in urls):
                raise ValueError('Published repository identity differs from the enrolled upstream; review provenance before upgrading')
        latest=data['version'] if kind=='npm' else data['info']['version']
        version(latest)
        if kind=='npm':
            if data.get('name')!=package or data.get('deprecated'): raise ValueError('Latest npm release is deprecated or has wrong identity')
            dist=data['dist']; integrity=dist.get('integrity','')
            match=re.fullmatch(r'sha512-([A-Za-z0-9+/=]+)',integrity)
            if not match or len(base64.b64decode(match[1],validate=True))!=64: raise ValueError('Registry omitted verified SHA512 integrity')
            artifact={'url':dist['tarball'],'algorithm':'sha512','digest':base64.b64decode(match[1],validate=True).hex(),'hosts':hosts}
            constraint=data.get('engines',{}).get('node')
            migration_flags=data.get('peerDependencies',{})
        else:
            candidates=[item for item in data['urls'] if item.get('packagetype')=='bdist_wheel' and item.get('filename','').endswith('-py3-none-any.whl') and not item.get('yanked')]
            if not candidates: candidates=[item for item in data['urls'] if item.get('packagetype')=='sdist' and not item.get('yanked')]
            if len(candidates)!=1: raise ValueError('No unique non-yanked portable wheel/source distribution to stage')
            selected=candidates[0]
            artifact={'url':selected['url'],'algorithm':'sha256','digest':selected['digests']['sha256'],'hosts':hosts}
            constraint=selected.get('requires_python') or data['info'].get('requires_python')
            migration_flags={}
        validate_url(artifact['url'],hosts)
        return {'latest':latest,'source_url':url,'artifact':artifact,'runtime_constraint':constraint,'migration_flags':migration_flags,**release_notes(source,latest)}
    repository=source.get('repository','')
    if kind=='blender': repository='blender/blender'
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+',repository): raise ValueError('Invalid public GitHub repository')
    base='https://api.github.com/repos/'+repository
    if kind=='blender':
        url=base+'/tags?per_page=100'
        tags=json.loads(fetch(url,['api.github.com']))
        stable=[]
        for item in tags:
            try: stable.append((version(item['name']),item['name'].lstrip('v')))
            except ValueError: continue
        if not stable: raise ValueError('No stable Blender version in official mirror tags')
        latest=max(stable)[1]
        return {'latest':latest,'source_url':url,'artifact':{'kind':'blender','version':latest},'migration_flags':{}}
    url=base+'/releases/latest'
    data=json.loads(fetch(url,['api.github.com']))
    if data.get('draft') or data.get('prerelease'): raise ValueError('Expected latest stable published release')
    latest=data['tag_name'].lstrip('v'); version(latest)
    asset_name=source.get('asset','').format(version=latest,tag=data['tag_name'])
    assets=[a for a in data.get('assets',[]) if a['name']==asset_name]
    if len(assets)!=1: raise ValueError('Configured platform release asset is missing/ambiguous')
    item=assets[0]; digest=item.get('digest','')
    if not re.fullmatch(r'sha256:[0-9a-f]{64}',digest): raise ValueError('Release asset lacks upstream SHA256 digest; configure a verified provider')
    hosts=['github.com','release-assets.githubusercontent.com','objects.githubusercontent.com']
    validate_url(item['browser_download_url'],hosts)
    return {'latest':latest,'source_url':url,'artifact':{'url':item['browser_download_url'],'algorithm':'sha256','digest':digest[7:],'hosts':hosts},'migration_flags':{}}


def compatibility(tool,current,release):
    have,want=version(current['version']),version(release['latest'])
    if want<=have: return None
    rule=tool['compatibility']
    if rule=='manual' or (rule=='same_major' and (want[0]!=have[0] or (have[0]==0 and want[:2]!=have[:2]))) or (rule=='same_minor' and want[:2]!=have[:2]):
        return 'Release crosses the configured compatibility boundary; migration review required'
    changed_peers=[name for name,requirement in release.get('migration_flags',{}).items() if current.get('peer_dependencies',{}).get(name)!=requirement]
    if changed_peers: return 'Changed peer dependencies need compatibility review: '+', '.join(changed_peers)
    warnings=[entry for entry in release.get('notes_review',[]) if have<version(entry['version'])<=want]
    if release.get('notes_truncated') and have<version(release['notes_oldest_reviewed']):
        return 'Bounded release-note coverage does not reach the installed version; review the remaining migration history'
    if warnings:
        return 'Upstream migration/breaking-change notes require review: '+release['notes_url']+' ('+', '.join(entry['version'] for entry in warnings)+')'
    constraint=release.get('runtime_constraint')
    if constraint:
        if not current.get('runtime_version'): return 'Latest release requires runtime '+constraint+'; installed runtime is unverified'
        try: okay=satisfies(current['runtime_version'],constraint,tool['source']['kind'])
        except ValueError as error: return str(error)
        if not okay: return 'Installed runtime '+current['runtime_version']+' conflicts with latest requirement '+constraint
    return None

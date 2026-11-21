"""Integrity-checked download only. No installation scripts, extraction or process replacement."""
import hashlib
import json
from pathlib import Path
import re
import time
import urllib.request
import urllib.parse

from providers import Redirects, fetch, validate_url


def artifact_metadata(artifact):
    if artifact.get('kind')!='blender': return artifact
    release=artifact['version']
    if not re.fullmatch(r'\d+\.\d+\.\d+',release): raise ValueError('Invalid Blender artifact version')
    import platform
    machine=platform.machine().lower()
    if platform.system()!='Windows' or machine not in ('amd64','x86_64'):
        raise ValueError('Blender automatic staging currently supports Windowsx64; configure a platform-specific provider')
    series='.'.join(release.split('.')[:2])
    name='blender-'+release+'-windows-x64.zip'
    base='https://download.blender.org/release/Blender'+series+'/'
    sums=fetch(base+'blender-'+release+'.sha256',['download.blender.org']).decode('ascii')
    matches=re.findall(r'^([0-9a-fA-F]{64})\s+\*?'+re.escape(name)+r'\s*$',sums,re.M)
    if len(matches)!=1: raise ValueError('Official Blender checksum missing/ambiguous')
    return {'url':base+name,'algorithm':'sha256','digest':matches[0].lower(),'hosts':['download.blender.org']}


def download(policy,tool,metadata,lease):
    artifact=artifact_metadata(metadata['artifact'])
    validate_url(artifact['url'],artifact['hosts'])
    algorithm=artifact['algorithm']
    length={'sha256':64,'sha512':128}.get(algorithm)
    if not length or not re.fullmatch('[0-9a-f]{'+str(length)+'}',artifact['digest']):
        raise ValueError('Invalid upstream artifact digest')
    root=Path(policy['state_dir'])/'downloads'/tool['id']/metadata['latest']/lease
    root.mkdir(parents=True,exist_ok=False)
    partial=root/'artifact.partial'
    name=urllib.parse.unquote(urllib.parse.urlsplit(artifact['url']).path.rsplit('/',1)[-1])
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.+-]{0,255}',name): raise ValueError('Artifact has no safe archive filename')
    target=root/name
    digest=hashlib.new(algorithm)
    opener=urllib.request.build_opener(Redirects(artifact['hosts']))
    request=urllib.request.Request(artifact['url'],headers={'User-Agent':'Kritic-tool-updates/1'})
    total=0
    started=time.monotonic()
    with opener.open(request,timeout=8) as response, partial.open('xb') as output:  # nosec B310
        declared=response.headers.get('Content-Length')
        if declared and int(declared)>policy['max_download_bytes']: raise ValueError('Artifact exceeds configured download limit')
        while True:
            if time.monotonic()-started>300: raise ValueError('Artifact download deadline exceeded')
            block=response.read(65536)
            if not block: break
            total+=len(block)
            if total>policy['max_download_bytes']: raise ValueError('Artifact exceeds configured download limit')
            output.write(block); digest.update(block)
    if total==0 or digest.hexdigest()!=artifact['digest']: raise ValueError('Downloaded artifact failed upstream integrity verification')
    partial.replace(target)
    receipt={'path':str(target),'size':total,'algorithm':algorithm,'digest':digest.hexdigest(),
             'version':metadata['latest'],'source_url':metadata['source_url'],'download_url':artifact['url'],
             'verified_at':time.time(),'active':False}
    (root/'receipt.json').write_text(json.dumps(receipt,indent=2),encoding='utf8')
    return receipt

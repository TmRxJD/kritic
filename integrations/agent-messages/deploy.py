"""Explicit additive enrollment; keeps existing hooks and native plugins."""
import argparse
import copy
import json
import os
from pathlib import Path
import shutil
import shlex
import subprocess  # nosec B404


def install(runtime, codex, claude, hermes=None, replace_runtime=None):
    source = Path(__file__).resolve().parent
    runtime = Path(runtime).resolve()
    runtime.mkdir(parents=True, exist_ok=True)
    previous = Path(replace_runtime).resolve() if replace_runtime else None
    for name in ('hook.py', 'contract.py', 'hermes_plugin.py', 'README.md'):
        target = runtime / name
        if target.exists() and target.read_bytes() != (source / name).read_bytes():
            raise ValueError(f'Refusing different installed runtime: {target}; choose a new version directory')
        if not target.exists():
            shutil.copy2(source / name, target)
    marker = {'integration':'kritic-agent-message-contract','schema_version':1}
    (runtime/'MESSAGE-CONTRACT.json').write_text(json.dumps(marker),encoding='utf8')
    updates = []
    for host, path in [('codex', Path(codex)/'hooks.json'), ('claude', Path(claude)/'settings.json')]:
        before = path.read_bytes() if path.exists() else None
        data = json.loads(before) if before else {}
        groups = data.setdefault('hooks', {}).setdefault('PreToolUse', [])
        retained = []
        for group in groups:
            kept = []
            for item in group.get('hooks', []):
                try:
                    tokens = shlex.split(item.get('command',''))
                except ValueError:
                    tokens = []
                owned = False
                if len(tokens) == 4 and tokens[:2] == ['python','-I'] and tokens[3] == host:
                    script = Path(tokens[2]).resolve()
                    tag = script.parent/'MESSAGE-CONTRACT.json'
                    managed = tag.is_file() and tag.stat().st_size < 1024 and json.loads(tag.read_text(encoding='utf8')) == marker
                    owned = script.name == 'hook.py' and (managed or (previous and script.parent == previous))
                    if owned and script.parent != runtime and script.parent != previous:
                        raise ValueError(f'Existing message contract at {script.parent}; use --replace-runtime to migrate its exact hooks')
                if not owned:
                    kept.append(item)
            if kept:
                retained.append(dict(group,hooks=kept))
        groups[:] = retained
        command = f'python -I "{(runtime / "hook.py").as_posix()}" {host}'
        if not any(item.get('command') == command for group in groups for item in group.get('hooks', [])):
            groups.append({'hooks':[{'type':'command','command':command,'timeout':10}]})
        updates.append((path,before,(json.dumps(data,indent=2)+'\n').encode('utf8')))
    if hermes:
        import yaml
        home = Path(hermes)
        path = home / 'config.yaml'
        before = path.read_bytes()
        raw = before.decode('utf8')
        expected = yaml.safe_load(raw)
        expected = copy.deepcopy(expected)
        plugins = expected.setdefault('plugins', {})
        if 'agent-message-contract' in plugins.get('disabled', []):
            raise ValueError('Hermes explicitly disables agent-message-contract')
        enabled = plugins.setdefault('enabled', [])
        if 'agent-message-contract' not in enabled:
            enabled.insert(0,'agent-message-contract')
            newline = '\r\n' if '\r\n' in raw else '\n'
            marker = 'plugins:' + newline + '  enabled:' + newline
            if raw.count(marker) != 1:
                raise ValueError('Unrecognized Hermes YAML plugin layout; configure explicitly')
            raw = raw.replace(marker,marker+'    - agent-message-contract'+newline,1)
        if yaml.safe_load(raw) != expected:
            raise ValueError('Unexpected Hermes configuration change')
        plugin = runtime / 'hermes-plugin'
        plugin.mkdir(exist_ok=True)
        (plugin/'__init__.py').write_bytes((source/'hermes_plugin.py').read_bytes())
        (plugin/'contract.json').write_text(json.dumps({'hook':(runtime/'hook.py').as_posix()}),encoding='utf8')
        (plugin/'plugin.yaml').write_text('name: agent-message-contract\nversion: 0.1.0\ndescription: Compact typed agent messages.\nhooks:\n  - pre_tool_call\n',encoding='utf8')
        destination = home/'plugins/agent-message-contract'
        create = not destination.exists()
        if destination.exists():
            if destination.resolve() != plugin:
                if not previous or destination.resolve() != previous/'hermes-plugin':
                    raise ValueError('Hermes message plugin already points elsewhere; select its exact --replace-runtime')
                backup = destination.with_name(destination.name+'.previous-'+previous.name)
                if backup.exists():
                    raise ValueError('Previous Hermes plugin link backup already exists; inspect before retrying')
                destination.rename(backup)
                create = True
        if create:
            destination.parent.mkdir(parents=True,exist_ok=True)
            if os.name == 'nt':
                shell = Path(os.environ.get('SystemRoot','C:/Windows'))/'System32/WindowsPowerShell/v1.0/powershell.exe'
                subprocess.run(  # nosec B603
                    [str(shell),'-NoProfile','-NonInteractive','-Command',
                     "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:MESSAGE_LINK -Target ([WildcardPattern]::Escape($env:MESSAGE_TARGET)) | Out-Null"],
                    env=dict(os.environ,MESSAGE_LINK=str(destination),MESSAGE_TARGET=str(plugin)),check=True,creationflags=subprocess.CREATE_NO_WINDOW)
            else:
                destination.symlink_to(plugin,target_is_directory=True)
        updates.append((path,before,raw.encode('utf8')))
    changed = []
    try:
        for path,before,after in updates:
            if (path.read_bytes() if path.exists() else None) != before:
                raise ValueError(f'Configuration changed: {path}')
            path.parent.mkdir(parents=True,exist_ok=True)
            temporary = path.with_name(path.name+'.message-contract-new')
            with temporary.open('xb') as stream:
                stream.write(after)
            if (path.read_bytes() if path.exists() else None) != before:
                raise ValueError(f'Configuration changed before replacement: {path}')
            os.replace(temporary,path)
            changed.append((path,before,after))
    except BaseException:
        for path,before,after in reversed(changed):
            if path.read_bytes() == after:
                if before is None:
                    path.unlink()
                else:
                    path.write_bytes(before)
        raise
    return {'hook':(runtime/'hook.py').as_posix(),'configs':[str(p) for p,_,_ in updates]}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--runtime',required=True)
    parser.add_argument('--codex-home',default=str(Path.home()/'.codex'))
    parser.add_argument('--claude-home',default=str(Path.home()/'.claude'))
    parser.add_argument('--hermes-home')
    parser.add_argument('--replace-runtime',help='Explicitly replace hooks owned by this previous runtime; preserve unrelated hooks')
    args = parser.parse_args()
    print(json.dumps(install(args.runtime,args.codex_home,args.claude_home,args.hermes_home,args.replace_runtime)))

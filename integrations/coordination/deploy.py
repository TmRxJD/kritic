"""Install the shared coordinator and additive Codex/Claude/Hermes adapters."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess  # nosec B404

FILES = ('coordinator.py', 'registry.py', 'paths.py', 'hooks.py', 'evaluate.py', 'transfers.py', 'recovery.py', 'hermes_plugin.py', 'README.md')


def install(runtime, codex, claude, hermes=None, hermes_kev_exe=None):
    source = Path(__file__).resolve().parent
    runtime = Path(runtime).resolve()
    runtime.mkdir(parents=True, exist_ok=True)
    hashes = {}
    for name in FILES:
        src, dst = source / name, runtime / name
        if dst.exists() and dst.read_bytes() != src.read_bytes():
            raise ValueError(f'Refusing to overwrite a different runtime: {dst}')
        if not dst.exists():
            shutil.copy2(src, dst)
        hashes[name] = hashlib.sha256(dst.read_bytes()).hexdigest()
    coordinator = runtime / 'coordinator.py'
    updates = {}
    for host, config in (('codex', Path(codex) / 'hooks.json'), ('claude', Path(claude) / 'settings.json')):
        before = config.read_bytes() if config.exists() else None
        data = json.loads(before) if before else {}
        hooks = data.setdefault('hooks', {})
        events = {'SessionStart': 'session', 'UserPromptSubmit': 'prompt', 'PreToolUse': 'pretool',
                  'SessionEnd': 'end', 'SubagentStart': 'subagent', 'SubagentStop': 'subagent_end', 'Stop':'boundary'}
        for event, action in events.items():
            command = f'python -I "{coordinator.as_posix()}" hook {host} {action}'
            groups = hooks.setdefault(event, [])
            if not any(item.get('command') == command for group in groups for item in group.get('hooks', [])):
                groups.append({'hooks': [{'type': 'command', 'command': command, 'timeout': 30}]})
        updates[config] = (before, (json.dumps(data, indent=2) + '\n').encode())
    hermes_result = 'not requested'
    if hermes:
        import yaml
        home = Path(hermes)
        config = home / 'config.yaml'
        if not config.is_file():
            raise ValueError('Hermes config.yaml not found; pass the actual Hermes home')
        plugin = runtime / 'hermes-plugin'
        plugin.mkdir(exist_ok=True)
        (plugin / '__init__.py').write_bytes((source / 'hermes_plugin.py').read_bytes())
        (plugin / 'coordinator.json').write_text(json.dumps({'coordinator': coordinator.as_posix()}), encoding='utf8')
        (plugin / 'plugin.yaml').write_text('name: worktree-coordination\nversion: 0.1.0\ndescription: Shared worktree intent and scope coordination.\nhooks:\n  - pre_tool_call\n  - on_session_end\n', encoding='utf8')
        destination = home / 'plugins/worktree-coordination'
        if destination.exists():
            if destination.resolve() != plugin:
                raise ValueError('Hermes coordination plugin already exists at another location')
        elif os.name == 'nt':
            destination.parent.mkdir(parents=True, exist_ok=True)
            shell = Path(os.environ.get('SystemRoot', 'C:/Windows')) / 'System32/WindowsPowerShell/v1.0/powershell.exe'
            env = dict(os.environ, KRITIC_COORD_LINK=str(destination), KRITIC_COORD_TARGET=str(plugin))
            # Fixed script, absolute trusted interpreter; paths are data operands.
            subprocess.run(  # nosec B603
                [str(shell), '-NoProfile', '-NonInteractive', '-Command',
                 "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:KRITIC_COORD_LINK -Target ([WildcardPattern]::Escape($env:KRITIC_COORD_TARGET)) | Out-Null"],
                env=env, check=True, creationflags=subprocess.CREATE_NO_WINDOW)
        else:
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.symlink_to(plugin, target_is_directory=True)
        before = config.read_bytes()
        raw = before.decode('utf8')
        existing = yaml.safe_load(raw)
        expected = copy.deepcopy(existing)
        enabled = expected.setdefault('plugins', {}).setdefault('enabled', [])
        if 'worktree-coordination' in expected['plugins'].get('disabled', []):
            raise ValueError('Hermes explicitly disables worktree-coordination; resolve that configuration before activation')
        if 'worktree-coordination' not in enabled:
            enabled.insert(0, 'worktree-coordination')
            newline = '\r\n' if '\r\n' in raw else '\n'
            marker = 'plugins:' + newline + '  enabled:' + newline
            if raw.count(marker) != 1:
                raise ValueError('Hermes plugin block has a different layout; preserve it and configure the plugin explicitly')
            raw = raw.replace(marker, marker + '    - worktree-coordination' + newline, 1)
        if hermes_kev_exe:
            executable = Path(hermes_kev_exe)
            if not executable.is_absolute() or not executable.is_file():
                raise ValueError('Hermes Kev Tools executable must be an existing absolute path')
            servers = expected.setdefault('mcp_servers', {})
            if 'kev-tools' not in servers:
                servers['kev-tools'] = {'command': executable.as_posix(), 'args': [], 'timeout': 120}
                newline = '\r\n' if '\r\n' in raw else '\n'
                marker = 'mcp_servers:' + newline
                if raw.count(marker) != 1:
                    raise ValueError('Cannot locate the Hermes MCP block without rewriting other settings')
                block = ('  kev-tools:' + newline + '    command: ' + json.dumps(executable.as_posix()) + newline
                         + '    args: []' + newline + '    timeout: 120' + newline)
                raw = raw.replace(marker, marker + block, 1)
            elif servers['kev-tools'].get('command') != executable.as_posix():
                raise ValueError('Existing Hermes Kev Tools registration needs an explicit preserved update')
        if yaml.safe_load(raw) != expected:
            raise ValueError('Unexpected Hermes configuration mutation')
        updates[config] = (before, raw.encode())
        hermes_result = str(destination)
    changed = {}
    try:
        for config, (before, after) in updates.items():
            if (config.read_bytes() if config.exists() else None) != before:
                raise ValueError(f'Configuration changed concurrently: {config}')
            config.parent.mkdir(parents=True, exist_ok=True)
            temporary = config.with_name(config.name + '.coordination-new')
            with temporary.open('xb') as stream:
                stream.write(after)
            if (config.read_bytes() if config.exists() else None) != before:
                raise ValueError(f'Configuration changed before replacement: {config}')
            os.replace(temporary, config)
            changed[config] = after
    except BaseException:
        for config, after in changed.items():
            if config.read_bytes() == after:
                before = updates[config][0]
                if before is not None:
                    config.write_bytes(before)
                else:
                    config.unlink()
        raise
    return {'coordinator': coordinator.as_posix(), 'hashes': hashes, 'configs': [str(p) for p in updates], 'hermes_plugin': hermes_result}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--runtime', required=True)
    parser.add_argument('--codex-home', default=str(Path.home() / '.codex'))
    parser.add_argument('--claude-home', default=str(Path.home() / '.claude'))
    parser.add_argument('--hermes-home')
    parser.add_argument('--hermes-kev-exe', help='Also add the Kev Tools MCP entry to Hermes if it is absent')
    args = parser.parse_args()
    print(json.dumps(install(args.runtime, args.codex_home, args.claude_home, args.hermes_home, args.hermes_kev_exe), indent=2))

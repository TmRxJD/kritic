"""Deterministic tests for tool-router.py on a temp project; touches no real config. Run: python test_tool_router.py"""
import json, os, subprocess, sys, shutil, tempfile
from pathlib import Path
R = str(Path(__file__).resolve().parent / 'tool-router.py')
T = Path(os.environ.get('TR_TEST_DIR') or tempfile.mkdtemp(prefix='tool-router-test-'))
W = T / 'w'; shutil.rmtree(W, ignore_errors=True)
proj = W / 'proj'; (proj / '.git/info').mkdir(parents=True); (proj / 'game.uproject').write_text('{}')
cl = W / 'claude'; (cl / 'skills/oldskill').mkdir(parents=True); (cl / 'skills/oldskill/SKILL.md').write_text('---\ndescription: old\n---\n')
(cl / 'settings.json').write_text(json.dumps({'hooks': {'PreToolUse': [{'matcher': 'Bash', 'hooks': [{'type': 'command', 'command': 'rtk hook claude'}]}]}, 'enabledPlugins': {}}))
cj = W / 'claude.json'
cj.write_text(json.dumps({'mcpServers': {'kritic': {'command': 'k'}}, 'projects': {str(proj).replace('\\', '/'): {'mcpServers': {'unreal-mcp': {'command': 'u'}}}}}))
cx = W / 'codex'; cx.mkdir()
(cx / 'config.toml').write_text('model="x"\n[mcp_servers.blender]\ncommand = "b"\n[mcp_servers.blender.env]\nA="1"\n[mcp_servers.kritic]\ncommand = "k"\n')
(cx / 'hooks.json').write_text(json.dumps({'hooks': {'PreToolUse': [{'matcher': 'Bash', 'hooks': [{'type': 'command', 'command': 'rtk hook claude'}]}]}}))
fake = W / 'fakeclaude.py'
fake.write_text("import sys;open(r'%s','a').write(' '.join(sys.argv[1:])+'\\n')" % (W / 'claude-calls.txt').as_posix())
ENV = dict(os.environ, TOOL_ROUTER_HOME=str(W / 'state'), TOOL_ROUTER_CLAUDE_DIR=str(cl), TOOL_ROUTER_CLAUDE_JSON=str(cj),
           TOOL_ROUTER_CODEX_DIR=str(cx), TOOL_ROUTER_CLAUDE_CMD=f'{sys.executable} {fake}')

def run(*a, inp=None):
    r = subprocess.run([sys.executable, '-I', R, *a], input=inp, capture_output=True, text=True, env=ENV, cwd=proj, encoding='utf-8')
    return r.stdout.strip() + r.stderr.strip()
def hook(event, tool=None, ti=None, host='claude', sid='S1'):
    return run('hook', host, event, inp=json.dumps({'session_id': sid, 'cwd': str(proj), 'tool_name': tool, 'tool_input': ti or {}}))
def denied(o): return '"deny"' in o
n = 0; bad = False
def check(name, cond, info=''):
    global n, bad; n += 1; bad |= not cond
    print(('PASS ' if cond else 'FAIL ') + name + ('' if cond else ' :: ' + str(info)[:300]))

check('baseline', 'baseline' in run('baseline'))
c = run('catalog'); check('detect unreal', '[unreal] DETECTED' in c, c[:200])
o = hook('pretool', 'mcp__unreal-mcp__spawn'); check('deny w/o plan', denied(o) and 'plan' in o, o)
check('core mcp allowed w/o plan', hook('pretool', 'mcp__kritic__status') == '' and hook('pretool', 'mcp__kev-tools__digest') == '')
check('builtin allowed', hook('pretool', 'Bash', {'command': 'ls'}) == '')
check('prompt nags w/o plan', 'no tool plan' in hook('prompt'))
run('plan', 'unreal', '--session', 'S1', '--task', 't')
check('allow after plan', hook('pretool', 'mcp__unreal-mcp__spawn') == '')
o = hook('pretool', 'mcp__rea__open_binary'); check('deny out-of-plan pack', denied(o) and 'not in this session' in o, o)
check('codex host denies too', denied(hook('pretool', 'mcp__rea__x', host='codex')))
run('plan', '--none', '--session', 'S3'); check('plan --none still blocks gated', denied(hook('pretool', 'mcp__rea__x', sid='S3')))
check('plan --none allows core', hook('pretool', 'mcp__kritic__status', sid='S3') == '')
# new skills must be registered
(cl / 'skills/freshskill').mkdir(); (cl / 'skills/freshskill/SKILL.md').write_text('---\ndescription: fresh\n---\n')
check('detect unregistered', 'freshskill' in run('unregistered'), run('unregistered'))
o = hook('pretool', 'Skill', {'skill': 'freshskill'}); check('deny unregistered skill', denied(o) and 'register' in o, o)
check('session start lists unregistered', 'freshskill' in hook('session'))
check('prompt lists unregistered', 'freshskill' in hook('prompt'))
run('register', 'freshskill', '--core'); check('allow after register', hook('pretool', 'Skill', {'skill': 'freshskill'}) == '')
# presets
run('preset', 'set', 'ue-dev', '--packs', 'unreal', '--note', 'ue work'); check('preset listed', 'ue-dev' in run('preset', 'list'))
run('plan', '--preset', 'ue-dev', '--session', 'S2'); p2 = W / 'state/plans/S2.json'
check('preset plan file', p2.exists() and 'unreal' in p2.read_text())
check('preset plan allows', hook('pretool', 'mcp__unreal-mcp__x', sid='S2') == '')
# subsets: only matching skills / MCP tools of a pack
run('plan', 'unreal:ue-mat*', 'rea:list_*', 'mcp__claude-in-chrome__read_*', '--session', 'S4')
check('subset skill allowed', hook('pretool', 'Skill', {'skill': 'ue-materials'}, sid='S4') == '')
check('subset skill other denied', denied(hook('pretool', 'Skill', {'skill': 'ue-blueprints'}, sid='S4')))
check('subset mcp tool allowed', hook('pretool', 'mcp__rea__list_names', sid='S4') == '')
check('subset mcp other denied', denied(hook('pretool', 'mcp__rea__open_binary', sid='S4')))
check('mcp full-name glob', hook('pretool', 'mcp__claude-in-chrome__read_page', sid='S4') == '' and denied(hook('pretool', 'mcp__claude-in-chrome__navigate', sid='S4')))
lib = W / 'lib'
for s in ('ue-materials', 'ue-blueprints', 'ue-landscape'):
    (lib / s).mkdir(parents=True); (lib / s / 'SKILL.md').write_text('---' + chr(10) + 'description: x' + chr(10) + '---' + chr(10))
import importlib.util
os.environ.update(TOOL_ROUTER_HOME=ENV['TOOL_ROUTER_HOME'], TOOL_ROUTER_CLAUDE_DIR=str(cl), TOOL_ROUTER_CLAUDE_JSON=str(cj), TOOL_ROUTER_CODEX_DIR=str(cx))
(proj / '.claude').mkdir(exist_ok=True); (proj / 'game.uproject').unlink()   # no auto-detect: subset only
(proj / '.claude/tool-router.json').write_text(json.dumps({'on': ['unreal:ue-mat*', 'unreal:ue-land*']}))
spec = importlib.util.spec_from_file_location('trm', R); m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m); m.LIB = lib
sel = m.skill_selection(m.root_of(proj))
check('selection is a subset', sel == {'unreal': ['ue-land*', 'ue-mat*']}, sel)
check('subset selects only matching', [s for s in ('ue-materials', 'ue-blueprints', 'ue-landscape') if m.skill_in_sel(sel, s)] == ['ue-materials', 'ue-landscape'])
check('subset does not enable MCP pack', m.active_packs(m.root_of(proj)) == [])
(proj / 'game.uproject').write_text('{}'); (proj / '.claude/tool-router.json').unlink()
# MCP toggle through (fake) claude CLI
check('protected refuses', 'protected' in run('disable', 'kritic'))
run('disable', 'unreal-mcp'); calls = (W / 'claude-calls.txt').read_text()
check('disable -> claude mcp remove', 'mcp remove -s local unreal-mcp' in calls, calls)
check('library kept def', 'unreal-mcp' in (W / 'state/mcp-library.json').read_text())
cj.write_text(json.dumps({'mcpServers': {'kritic': {'command': 'k'}}}))
run('enable', 'unreal-mcp'); calls = (W / 'claude-calls.txt').read_text()
check('enable -> add-json local', 'mcp add-json -s local unreal-mcp' in calls, calls)
# Codex toggle + restore
run('disable', 'blender', '--host', 'codex'); t = (cx / 'config.toml').read_text()
check('codex enabled=false, env kept', 'enabled = false' in t and '[mcp_servers.blender.env]' in t, t)
check('codex protected', 'protected' in run('disable', 'kritic', '--host', 'codex'))
run('restore'); check('restore codex', 'enabled = false' not in (cx / 'config.toml').read_text())
# CLI enforcement through kritic cli_run
check('shell allowed before enforcement', hook('pretool', 'Bash', {'command': 'git status'}, sid='C1') == '')
run('cli', 'enforce', 'on'); hook('session', sid='C1'); hook('session', sid='C0')
o = hook('pretool', 'Bash', {'command': 'cat <<EOF x EOF'}, sid='C1'); check('shell denied once session enforced', denied(o) and 'cli_run' in o, o)
check('PowerShell denied too', denied(hook('pretool', 'PowerShell', {'command': 'ls'}, sid='C1')))
check('router bootstrap allowed', hook('pretool', 'Bash', {'command': 'python -I x/tool-router.py plan --none'}, sid='C1') == '')
check('kritic cli_run allowed', hook('pretool', 'mcp__kritic__cli_run', sid='C1') == '')
check('unmarked (older) session unaffected', hook('pretool', 'Bash', {'command': 'ls'}, sid='OLD') == '')
run('cli', 'bypass', 'on'); check('bypass lifts it', hook('pretool', 'Bash', {'command': 'ls'}, sid='C1') == ''); run('cli', 'bypass', 'off')
check('enforced again after bypass off', denied(hook('pretool', 'Bash', {'command': 'ls'}, sid='C1')))
run('cli', 'enforce', 'off')
# install / uninstall hooks
run('install'); h = (cl / 'settings.json').read_text()
check('install keeps rtk', 'rtk hook claude' in h and 'tool-router.py hook claude pretool' in h and 'Bash|PowerShell' in h and 'UserPromptSubmit' in h and 'SessionStart' in h)
check('install codex', 'hook codex pretool' in (cx / 'hooks.json').read_text())
run('uninstall'); s = (cl / 'settings.json').read_text(); check('uninstall clean', 'tool-router' not in s and 'rtk hook claude' in s)
# usage / audit
a = json.loads(run('usage', 'S1')); check('usage log', a['called'].get('mcp:unreal-mcp', 0) >= 1 and any('rea' in d for d in a['denied']), a)
print(f'{n} checks', 'FAILED' if bad else 'all passed'); sys.exit(1 if bad else 0)

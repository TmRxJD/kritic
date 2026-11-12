#!/usr/bin/env python3
"""Tool router for Claude Code + Codex: skills, MCP servers, plugins. See README.md.
Subsets: `plan unreal:ue-material*` / `plan rea:list_*` / `enable unreal:ue-material*` pick only matching skills/MCP tools.
Commands (use `tool:NAME` to force a single tool when a pack has the same name): catalog [PACK|--full] | status | plan [PACK|name ...] [--none] [--task T] [--session ID]
          enable|disable PACK|name [--project D] | sync [D] [--host H] | usage [SESSION] | audit [SESSION]
          install | uninstall | restore [ID] | backups | hook HOST EVENT | selftest
Env overrides (tests): TOOL_ROUTER_HOME (state/backups/usage), TOOL_ROUTER_CLAUDE_CMD, TOOL_ROUTER_DRY=1,
TOOL_ROUTER_CODEX_DIR, TOOL_ROUTER_CLAUDE_DIR, TOOL_ROUTER_CLAUDE_JSON."""
import fnmatch, glob, hashlib, json, os, re, shlex, shutil, subprocess, sys, time
from pathlib import Path

try: sys.stdout.reconfigure(encoding='utf-8')
except Exception: pass
HOME = Path.home()
CFGDIR = Path(__file__).resolve().parent
CFG = json.loads((CFGDIR / 'tools.json').read_text(encoding='utf8'))
STATE = Path(os.environ.get('TOOL_ROUTER_HOME', 'B:/AI/tool-router'))  # E: is full (2026-11-11); B: has space, not the scratch janitor dir
CLAUDE_DIR = Path(os.environ.get('TOOL_ROUTER_CLAUDE_DIR', HOME / '.claude'))
CLAUDE_JSON = Path(os.environ.get('TOOL_ROUTER_CLAUDE_JSON', HOME / '.claude.json'))
CODEX_DIR = Path(os.environ.get('TOOL_ROUTER_CODEX_DIR', HOME / '.codex'))
CLAUDE_CMD = [x.strip('"') for x in shlex.split(os.environ.get('TOOL_ROUTER_CLAUDE_CMD', 'claude'), posix=False)]
CLAUDE_CMD[0] = shutil.which(CLAUDE_CMD[0]) or CLAUDE_CMD[0]
LIB = Path(os.path.expanduser(CFG['skill_library']))
PROTECTED = CFG['protected_mcp']          # never toggled, always allowed
SCRIPT = str(Path(__file__).resolve()).replace('\\', '/')
HOOK_MARK = 'tool-router.py hook'

# ---------- helpers ----------
def jload(p, default=None):
    try: return json.loads(Path(p).read_text(encoding='utf8'))
    except Exception: return default
def jsave(p, obj):
    Path(p).parent.mkdir(parents=True, exist_ok=True); Path(p).write_text(json.dumps(obj, indent=1), encoding='utf8')
def norm(s): return re.sub(r'[^A-Za-z0-9_-]', '_', s)
def gmatch(name, pats): return any(fnmatch.fnmatch(name, p) or fnmatch.fnmatch(norm(name), p) for p in pats)
def root_of(d):
    d = Path(d).resolve()
    for p in [d, *d.parents]:
        if (p / '.git').exists() or (p / '.claude').is_dir(): return p
    return d
def pkey(root): return str(root).replace('\\', '/')
def short(s, n=70):
    s = re.sub(r'\s+', ' ', s or '').strip(); return s if len(s) <= n else s[:n - 1] + '…'
def frontmatter_desc(skill_md):
    try: t = Path(skill_md).read_text(encoding='utf8', errors='ignore')[:1500]
    except Exception: return ''
    m = re.search(r'^description:\s*(.+?)(?:\n[a-zA-Z_-]+:|\n---)', t, re.S | re.M)
    return short(m.group(1).strip().strip('"\'>|')) if m else ''

# ---------- registry (every skill/MCP must be routed) + presets ----------
def registry(): return jload(STATE / 'registry.json', {'skills': []}) or {'skills': []}
def disk_skills():
    out = set()
    for b in (CLAUDE_DIR / 'skills', CODEX_DIR / 'skills'):
        if b.is_dir(): out |= {e.name for e in b.iterdir() if (e / 'SKILL.md').exists()}
    return out
def unregistered(root=None):
    known = set(registry()['skills']); out = {s for s in disk_skills() - known if pack_of('skills', s) is None and s != 'skill-library'}
    if root and (root / '.claude/skills').is_dir():
        out |= {e.name for e in (root / '.claude/skills').iterdir() if (e / 'SKILL.md').exists() and e.name not in known and pack_of('skills', e.name) is None}
    return sorted(out)
def cmd_register(names, pack, core, root):
    """acknowledge new skills: assign a pack (adds the name to the pack's skill globs) or mark core."""
    if not names: names = unregistered(root)
    if not names: print('nothing unregistered'); return
    snapshot([CFGDIR / 'tools.json', STATE / 'registry.json'], 'register')
    if pack:
        if pack not in CFG['packs']: sys.exit(f'unknown pack {pack}; packs: {", ".join(CFG["packs"])}')
        g = CFG['packs'][pack].setdefault('skills', [])
        for n in names:
            if n not in g: g.append(n)
        jsave(CFGDIR / 'tools.json', CFG)
    elif not core: sys.exit('register needs --pack PACK (rarely used) or --core (always visible)')
    r = registry(); r['skills'] = sorted(set(r['skills']) | set(names)); jsave(STATE / 'registry.json', r)
    print(f'registered {names} -> {pack or "core"}. Pack skills become gated by plans; to also hide them from the skill list move the folder into {LIB}.')
def presets(root):
    g = CFG.get('presets', {}); p = (jload(root / '.claude/tool-router.json', {}) or {}).get('presets', {}); return {**g, **p}
def cmd_preset(a, root):
    sub = a[0] if a else 'list'; name = a[1] if len(a) > 1 else None
    if sub == 'list':
        for n, v in presets(root).items(): print(f'{n}: packs={v.get("packs", [])} tools={v.get("tools", [])} {v.get("note", "")}')
        return
    f = root / '.claude/tool-router.json'
    if sub == 'set':
        packs = [x for x in (opt(a, '--packs', '') or '').split(',') if x]; tools = [x for x in (opt(a, '--tools', '') or '').split(',') if x]; note = opt(a, '--note', '')
        bad = [x for x in packs if x.split(':')[0] not in CFG['packs']]
        if bad: sys.exit(f'unknown packs {bad}')
        snapshot([f], 'preset'); c = jload(f, {}) or {}; c.setdefault('presets', {})[name] = {'packs': packs, 'tools': tools, 'note': note}
        (root / '.claude').mkdir(exist_ok=True); jsave(f, c); print(f'preset {name} saved in {f}')
    elif sub == 'delete':
        snapshot([f], 'preset'); c = jload(f, {}) or {}; c.get('presets', {}).pop(name, None); jsave(f, c); print(f'preset {name} removed')
    else: sys.exit('preset list | set NAME --packs a,b [--tools x,y] [--note T] | delete NAME ; use with `plan --preset NAME`')

# ---------- packs ----------
def pack_of(kind, name):
    for pk, p in CFG['packs'].items():
        if gmatch(name, p.get(kind, [])): return pk
    return None
def is_core_mcp(server): return gmatch(server, PROTECTED + CFG['core_mcp'])
def detect(root):
    out = []
    for pk, p in CFG['packs'].items():
        hit = any(glob.glob(str(root / m)) for m in p.get('markers', []))
        hint = p.get('path_hint'); hit = hit or bool(hint and hint in pkey(root).lower())
        if hit: out.append(pk)
    return out
def forced(root):
    c = jload(root / '.claude/tool-router.json', {}); return {'on': set(c.get('on', [])), 'off': set(c.get('off', []))}
def active_packs(root):
    f = forced(root); return sorted((set(detect(root)) | {e for e in f['on'] if ':' not in e}) - f['off'])
def skill_selection(root):
    """pack -> None (every skill in the pack) or [globs] (only those skills). `PACK:glob` entries select a subset."""
    sel = {pk: None for pk in active_packs(root)}; f = forced(root)
    for e in sorted(f['on']):
        if ':' in e:
            pk, g = e.split(':', 1)
            if pk in CFG['packs'] and pk not in f['off'] and sel.get(pk, 0) is not None: sel.setdefault(pk, []).append(g)
    return sel
def skill_in_sel(sel, s):
    pk = pack_of('skills', s)
    return pk in sel and (sel[pk] is None or any(fnmatch.fnmatch(s, g) for g in sel[pk]))
def plan_allows(plan, kind, name, pack, tool):
    """plan entries: pack | server/skill name | `X:glob` subset (X = pack or server; glob over skill name or MCP tool short name) | full mcp__ tool name."""
    if pack in plan.get('packs', []) or name in plan.get('tools', []) or tool in plan.get('tools', []): return True
    short = tool.split('__', 2)[2] if kind == 'mcp' and tool.count('__') >= 2 else name
    for e in plan.get('tools', []):
        if ':' in e:
            left, g = e.split(':', 1)
            if left in (pack, name) and fnmatch.fnmatch(short, g): return True
        elif any(c in e for c in '*?') and fnmatch.fnmatch(tool, e): return True
    return False

# ---------- backups ----------
def snapshot(files, label):
    sid = time.strftime('%Y%m%d-%H%M%S') + '-' + label; d = STATE / 'backups' / sid; d.mkdir(parents=True, exist_ok=True)
    idx = {}
    for i, f in enumerate(files):
        f = Path(f)
        if f.exists(): shutil.copy2(f, d / f'{i}.bak'); idx[str(f)] = f'{i}.bak'
        else: idx[str(f)] = None   # did not exist: restore deletes it
    jsave(d / 'index.json', idx); return sid
def restore(sid=None):
    bs = sorted(p.name for p in (STATE / 'backups').glob('*') if (p / 'index.json').exists())
    if not bs: sys.exit('no backups')
    sid = sid or bs[-1]; d = STATE / 'backups' / sid; idx = jload(d / 'index.json')
    if idx is None: sys.exit(f'unknown backup {sid}')
    for f, b in idx.items():
        if b: shutil.copy2(d / b, f)
        elif Path(f).exists(): os.remove(f)
    print(f'restored {sid}: {len(idx)} files (junction links re-align on next `sync`)')

# ---------- Claude MCP / skills / plugins ----------
def claude_mcp_state(root):
    """name -> {'scope','def'} for MCP servers currently installed (user + local project)."""
    c = jload(CLAUDE_JSON, {}); out = {}
    for n, d in (c.get('mcpServers') or {}).items(): out[n] = {'scope': 'user', 'def': d}
    pj = c.get('projects') or {}
    for k in (pkey(root), str(root)):
        for n, d in (pj.get(k, {}).get('mcpServers') or {}).items(): out[n] = {'scope': 'local', 'def': d}
    for n, d in ((jload(root / '.mcp.json', {}) or {}).get('mcpServers') or {}).items(): out.setdefault(n, {'scope': 'project', 'def': d})
    return out
def mcp_library():
    lib = jload(STATE / 'mcp-library.json')
    if lib is None:   # seed from the 2026-11-11 removal backup
        lib = jload(CLAUDE_DIR / 'mcp-removed-20261111.json', {}) or {}; jsave(STATE / 'mcp-library.json', lib)
    return lib
def run_claude(args, dry=None):
    dry = os.environ.get('TOOL_ROUTER_DRY') == '1' if dry is None else dry
    if dry: print('DRY claude', ' '.join(args)); return 0
    r = subprocess.run(CLAUDE_CMD + args, capture_output=True, text=True)
    if r.returncode: print('claude', ' '.join(args[:3]), 'failed:', (r.stderr or r.stdout).strip()[:200], file=sys.stderr)
    return r.returncode
def mcp_disable(name, root, scope=None):
    if gmatch(name, PROTECTED): sys.exit(f'{name} is protected (kritic/kev-tools stay enabled)')
    st = claude_mcp_state(root).get(name)
    if not st: print(f'{name}: not installed'); return
    lib = mcp_library(); lib[name] = st['def']; jsave(STATE / 'mcp-library.json', lib)   # save BEFORE removing
    run_claude(['mcp', 'remove', '-s', scope or st['scope'], name]); print(f'{name}: disabled (definition kept in library; next session)')
def mcp_enable(name, root, scope='local'):
    d = mcp_library().get(name)
    if not d:
        if name in claude_mcp_state(root): print(f'{name}: already enabled'); return
        sys.exit(f'{name}: not in the library')
    if name in claude_mcp_state(root): print(f'{name}: already enabled'); return
    run_claude(['mcp', 'add-json', '-s', scope, name, json.dumps(d).replace('\\', '/')]); print(f'{name}: enabled at {scope} scope (next session)')
def skills_state(root):
    out = {}
    for base, st in [(CLAUDE_DIR / 'skills', 'on'), (root / '.claude/skills', 'on'), (LIB, 'off')]:
        if base.is_dir():
            for e in sorted(base.iterdir()):
                if (e / 'SKILL.md').exists() and e.name not in out: out[e.name] = (st, e)
    return out
def plugins_state():
    s = jload(CLAUDE_DIR / 'settings.json', {}).get('enabledPlugins', {}); inst = jload(CLAUDE_DIR / 'plugins/installed_plugins.json', {}).get('plugins', {})
    return {n: bool(s.get(n, False)) for n in sorted(set(s) | set(inst))}
def set_plugin(name, on):
    if name not in plugins_state(): sys.exit(f'unknown plugin {name}')
    f = CLAUDE_DIR / 'settings.json'; snapshot([f], 'plugin'); s = jload(f, {}); s.setdefault('enabledPlugins', {})[name] = on; jsave(f, s)
    print(f'{name}: enabledPlugins={on} (next session)')

# junction-based skill packs (project scope)
def junction(link, target):
    r = subprocess.run(['cmd', '/c', 'mklink', '/J', str(link), str(target)], capture_output=True, text=True)
    if r.returncode: raise RuntimeError((r.stderr or r.stdout).strip())
def sync_skills(root, sel):
    sk = root / '.claude/skills'; man = sk / '.skill-router.json'
    want = {s: LIB / s for s in (LIB.iterdir() if LIB.is_dir() else []) for s in [s.name] if (LIB / s / 'SKILL.md').exists() and skill_in_sel(sel, s)}
    have = (jload(man, {}) or {}).get('links', [])
    if not want and not have: return 0, 0
    sk.mkdir(parents=True, exist_ok=True); add = rem = 0
    for s in have:
        if s not in want and (sk / s).exists(): os.rmdir(sk / s); rem += 1
    for s, t in want.items():
        if not (sk / s).exists(): junction(sk / s, os.path.realpath(t)); add += 1
    jsave(man, {'links': sorted(want)})
    ex = root / '.git/info/exclude'
    if ex.parent.is_dir():
        t = ex.read_text(encoding='utf8') if ex.exists() else ''
        if '.claude/skills/' not in t: ex.write_text(t + ('\n' if t and not t.endswith('\n') else '') + '.claude/skills/\n', encoding='utf8')
    return add, rem

# ---------- Codex ----------
def codex_cfg(): return CODEX_DIR / 'config.toml'
def codex_blocks():
    """parse [mcp_servers.X] / [plugins."X"] tables: returns {('mcp'|'plugin', name): enabled}"""
    t = codex_cfg().read_text(encoding='utf8') if codex_cfg().exists() else ''; out = {}
    for m in re.finditer(r'^\[(mcp_servers\.([\w-]+)|plugins\."([^"]+)")\]\s*\n((?:(?!\[).*\n?)*)', t, re.M):
        kind, name = ('mcp', m.group(2)) if m.group(2) else ('plugin', m.group(3))
        out[(kind, name)] = not re.search(r'^enabled\s*=\s*false', m.group(4), re.M)
    return out
def codex_set(kind, name, on):
    if kind == 'mcp' and gmatch(name, PROTECTED): sys.exit(f'{name} is protected')
    f = codex_cfg(); snapshot([f], 'codex'); t = f.read_text(encoding='utf8')
    hdr = f'[mcp_servers.{name}]' if kind == 'mcp' else f'[plugins."{name}"]'
    m = re.search(re.escape(hdr) + r'\s*\n((?:(?!\[).*\n?)*)', t)
    if not m: sys.exit(f'codex {kind} {name}: not found in config.toml')
    body = re.sub(r'^enabled\s*=.*\n?', '', m.group(1), flags=re.M)
    body = ('' if on else 'enabled = false\n') + body
    f.write_text(t[:m.start(1)] + body + t[m.end(1):], encoding='utf8'); print(f'codex {kind} {name}: enabled={on} (next Codex session)')
def codex_skills():
    out = {}
    for base, st in [(CODEX_DIR / 'skills', 'on'), (CODEX_DIR / 'skills-disabled', 'off')]:
        if base.is_dir():
            for e in sorted(base.iterdir()):
                if (e / 'SKILL.md').exists(): out[e.name] = (st, e)
    return out
def codex_skill_set(name, on):
    s = codex_skills().get(name)
    if not s: sys.exit(f'codex skill {name} not found')
    src = s[1]; dst = (CODEX_DIR / ('skills' if on else 'skills-disabled')) / name
    if src.parent == dst.parent: print(f'codex skill {name}: already {"on" if on else "off"}'); return
    dst.parent.mkdir(exist_ok=True); shutil.move(str(src), str(dst)); print(f'codex skill {name}: enabled={on} (moved, not deleted; next session)')

# ---------- catalog ----------
def build_catalog(root, host='claude'):
    items = []   # dict(kind,name,state,desc,pack,host)
    def add(kind, name, state, desc, h): items.append(dict(kind=kind, name=name, state=state, desc=desc, pack=pack_of({'skill': 'skills', 'mcp': 'mcp', 'plugin': 'plugins'}[kind], name), host=h))
    mcp_desc = CFG.get('desc', {})
    if host in ('claude', 'all'):
        for n, (st, p) in skills_state(root).items(): add('skill', n, st, frontmatter_desc(p / 'SKILL.md'), 'claude')
        live = claude_mcp_state(root)
        for n in live: add('mcp', n, 'on', mcp_desc.get(n, ''), 'claude')
        for n in mcp_library():
            if n not in live: add('mcp', n, 'off', mcp_desc.get(n, ''), 'claude')
        for n, on in plugins_state().items(): add('plugin', n, 'on' if on else 'off', mcp_desc.get(n, ''), 'claude')
    if host in ('codex', 'all'):
        for n, (st, p) in codex_skills().items(): add('skill', n, st, frontmatter_desc(p / 'SKILL.md'), 'codex')
        for (k, n), on in codex_blocks().items(): add('mcp' if k == 'mcp' else 'plugin', n, 'on' if on else 'off', mcp_desc.get(n, ''), 'codex')
    return items
def catalog_text(root, host='claude', pack=None, full=False):
    items = build_catalog(root, host); act = active_packs(root); lines = []
    by = {}
    for it in items: by.setdefault(it['pack'] or '(core)', []).append(it)
    if pack and pack not in CFG['packs']:
        sys.exit(f'unknown pack {pack}; packs: {", ".join(CFG["packs"])}')
    for pk in ([pack] if pack else list(CFG['packs']) + ['(core)']):
        its = by.get(pk, [])
        if pk == '(core)' and not full and not pack:
            lines.append(f'core (always allowed, {len(its)} items: {sum(i["kind"]=="skill" for i in its)} skills, {sum(i["kind"]=="mcp" for i in its)} mcp, {sum(i["kind"]=="plugin" for i in its)} plugins) - `catalog (core)` lists them'); continue
        if pk != '(core)':
            p = CFG['packs'][pk]; on = sum(i['state'] == 'on' for i in its)
            lines.append(f'[{pk}] {"DETECTED " if pk in act else ""}{p["note"]} | {len(its)} items, {on} installed')
            if not (pack or full): continue
        for i in its: lines.append(f'  {i["kind"][0]}:{i["name"]} [{i["state"]}] {i["desc"] if (pack or full) else ""}'.rstrip())
    return '\n'.join(lines)

# ---------- plan (per-session declaration; live-enforced by the hook) ----------
def plan_path(sid): return STATE / 'plans' / (re.sub(r'[^\w-]', '_', sid) + '.json')
def cwd_path(cwd): return STATE / 'plans' / ('cwd-' + hashlib.md5(pkey(root_of(cwd)).lower().encode()).hexdigest()[:12] + '.json')
def load_plan(sid, cwd):
    p = jload(plan_path(sid)) if sid else None
    if p is None and cwd:
        p = jload(cwd_path(cwd))
        if p and time.time() - p.get('ts', 0) > 12 * 3600: p = None
    return p
def cmd_plan(args, session, task, root, none=False, preset=None):
    if preset:
        pr = presets(root).get(preset)
        if not pr: sys.exit(f'unknown preset {preset}; have {list(presets(root))}')
        args = list(args) + pr.get('packs', []) + pr.get('tools', []); none = none or not args
    packs = [a for a in args if a in CFG['packs']]; names = [a for a in args if a not in CFG['packs']]   # names: tools, skills, `X:glob` subsets, mcp__ globs
    plan = dict(ts=time.time(), packs=sorted(packs), tools=sorted(names), none=none, task=task or '', project=pkey(root))
    if session: jsave(plan_path(session), plan)
    jsave(cwd_path(root), plan)
    print(f'plan recorded (live): packs={plan["packs"]} tools={plan["tools"]}' + (' core-only' if none else '') + f' session={session or "-"}')

# ---------- hooks ----------
def log_usage(sid, rec):
    try:
        d = STATE / 'usage'; d.mkdir(parents=True, exist_ok=True); rec['ts'] = round(time.time(), 1)
        with open(d / (re.sub(r'[^\w-]', '_', sid or 'nosession') + '.jsonl'), 'a', encoding='utf8') as f: f.write(json.dumps(rec) + '\n')
    except Exception: pass
SHELL_TOOLS = ('Bash', 'PowerShell')
def cli_enforced(sid):
    """Only sessions that started after `cli enforce on` (they hold a marker, so their kritic has cli_run) are enforced."""
    if os.environ.get('TOOL_ROUTER_CLI_BYPASS') == '1' or (STATE / 'cli-bypass').exists(): return False
    return bool(sid) and (STATE / 'cli-sessions' / (re.sub(r'[^\w-]', '_', sid) + '.on')).exists()
def cli_msg(cmd):
    return ('Tool router: shell commands must go through kritic. Call `mcp__kritic__cli_run` with {"program": "git", "args": ["status"], "cwd": "..."} '
            '(one array element per argument, optional "stdin" text, or "script": {"language": "python|node|powershell|bash", "code": "..."} for multi-line code; no heredocs, sed or -c strings). '
            '`"dry_run": true` shows the resolved command. Use Read/Edit/Write for file content. Emergency bypass: create ' + str(STATE / 'cli-bypass').replace('\\', '/') + '.')
def cmd_cli(a):
    sub = a[0] if a else 'status'; flag = STATE / 'cli-enforce'
    if sub == 'enforce':
        if (a[1] if len(a) > 1 else '') == 'on': flag.parent.mkdir(parents=True, exist_ok=True); flag.write_text(str(time.time())); print('cli enforcement ON for sessions started from now on (running sessions are unaffected)')
        else:
            if flag.exists(): flag.unlink()
            print('cli enforcement OFF for new sessions')
    elif sub == 'bypass':
        b = STATE / 'cli-bypass'
        if (a[1] if len(a) > 1 else '') == 'on': b.write_text('1'); print('bypass ON: shell tools allowed everywhere')
        else:
            if b.exists(): b.unlink()
            print('bypass OFF')
    else: print('enforce', 'on' if flag.exists() else 'off', '| bypass', 'on' if (STATE / 'cli-bypass').exists() else 'off', '| enforced sessions', len(list((STATE / 'cli-sessions').glob('*.on'))) if (STATE / 'cli-sessions').is_dir() else 0)
def classify(ev):
    tool = ev.get('tool_name', ''); ti = ev.get('tool_input') or {}
    if tool == 'Skill':
        n = str(ti.get('skill', '')).split(':')[-1]; return 'skill', n, pack_of('skills', n), False
    m = re.match(r'mcp__(.+?)__', tool)
    if m: s = m.group(1); return 'mcp', s, pack_of('mcp', s), is_core_mcp(s)
    return 'builtin', tool, None, True   # shell tools are handled separately in hook()
def deny_msg(kind, name, pack, plan):
    what = f'{kind} "{name}"' + (f' (pack: {pack})' if pack else ' (uncategorized)')
    cmd = f'python -I {SCRIPT} plan {pack or name} --session {{SID}} --task "<one line>"'
    if plan is None:
        return f'Tool router: no tool plan declared for this session yet, so {what} is blocked. Declare it first: `{cmd}`; use `plan --none --session {{SID}}` if only core tools are needed. `python -I {SCRIPT} catalog` lists packs.'
    return f'Tool router: {what} is not in this session\'s plan (packs={plan["packs"]}, tools={plan["tools"]}). If the task needs it, extend the plan: `{cmd}`.'
def hook(host, event):
    try: ev = json.loads(sys.stdin.read() or '{}')
    except Exception: return
    sid = ev.get('session_id') or ev.get('sessionId') or ''; cwd = ev.get('cwd') or os.getcwd()
    out = {}
    try:
        if event == 'session':
            root = root_of(cwd); 
            try: do_sync(root, host)
            except Exception as e: log_usage(sid, {'ev': 'sync_error', 'err': str(e)[:200]})
            if (STATE / 'cli-enforce').exists() and sid:
                (STATE / 'cli-sessions').mkdir(parents=True, exist_ok=True); (STATE / 'cli-sessions' / (re.sub(r'[^\w-]', '_', sid) + '.on')).write_text('1')
            ctx = (f'Tool router active (session {sid}). Skills/MCP/plugins beyond core are gated.' + (' Shell commands (Bash/PowerShell) are DENIED: run every command through mcp__kritic__cli_run (program+args, stdin, or script).' if (STATE / 'cli-enforce').exists() else '') + f' Before using any non-core tool declare your set: '
                   f'`python -I {SCRIPT} plan <pack...> --session {sid} --task "<one line>"` (or `plan --none --session {sid}`). '
                   f'Catalog: `python -I {SCRIPT} catalog` / `catalog <pack>`. Detected for this project: {active_packs(root) or "none"}. Presets (`plan --preset NAME`): {list(presets(root)) or "none"}.' + (f' NEW UNREGISTERED SKILLS {unregistered(root)}: run `python -I {SCRIPT} register <name> --pack <pack>|--core` for each before use.' if unregistered(root) else ''))
            out = {'hookSpecificOutput': {'hookEventName': 'SessionStart', 'additionalContext': ctx}}
        elif event == 'prompt':
            un = unregistered(root_of(cwd))
            if un: out = {'hookSpecificOutput': {'hookEventName': 'UserPromptSubmit', 'additionalContext': f'Tool router: new unregistered skills {un}; route them with `python -I {SCRIPT} register <name> --pack <pack>|--core`.'}}
            elif load_plan(sid, cwd) is None:
                out = {'hookSpecificOutput': {'hookEventName': 'UserPromptSubmit', 'additionalContext': f'Tool router: no tool plan yet. Before non-core skills/MCP, run `python -I {SCRIPT} plan <pack...|--none> --session {sid} --task "<line>"`.'}}
        elif event == 'pretool':
            kind, name, pack, core = classify(ev)
            if kind == 'builtin' and name in SHELL_TOOLS:
                cmdline = str((ev.get('tool_input') or {}).get('command', ''))
                if 'tool-router.py' in cmdline or not cli_enforced(sid): return          # router bootstrap always allowed
                log_usage(sid, {'ev': 'deny', 'kind': 'shell', 'name': name, 'why': 'cli_run'})
                print(json.dumps({'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny', 'permissionDecisionReason': cli_msg(cmdline)}})); return
            okrec = {'ev': 'call', 'kind': kind, 'name': name, 'pack': pack, 'tool': ev.get('tool_name'), 'host': host}
            if kind == 'skill' and pack is None and name in unregistered(root_of(cwd)):
                msg = f'Tool router: skill "{name}" is new and not routed yet. Register it first: `python -I {SCRIPT} register {name} --pack <pack> | --core` (see `catalog`).'
                log_usage(sid, {'ev': 'deny', 'kind': kind, 'name': name, 'pack': None, 'why': 'unregistered'})
                print(json.dumps({'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny', 'permissionDecisionReason': msg}})); return
            if core or (kind == 'skill' and pack is None):
                if kind != 'builtin': log_usage(sid, okrec)
                return
            plan = load_plan(sid, cwd)
            if plan and plan_allows(plan, kind, name, pack, ev.get('tool_name', '')):
                log_usage(sid, okrec); return
            if plan and plan.get('none') and not plan.get('packs') and not plan.get('tools'): pass   # core-only plan: gated tools still blocked
            msg = deny_msg(kind, name, pack, plan).replace('{SID}', sid or '<session>')
            log_usage(sid, {'ev': 'deny', 'kind': kind, 'name': name, 'pack': pack})
            out = {'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny', 'permissionDecisionReason': msg}}
    except Exception as e: log_usage(sid, {'ev': 'hook_error', 'err': str(e)[:200]}); return   # fail open
    if out: print(json.dumps(out))

# ---------- sync / install ----------
def do_sync(root, host='claude'):
    packs = active_packs(root); res = {}
    if host == 'claude':
        res['skills'] = sync_skills(root, skill_selection(root))
        live = claude_mcp_state(root); lib = mcp_library()
        for n in lib:
            if n in live or gmatch(n, PROTECTED): continue
            if pack_of('mcp', n) in packs: mcp_enable(n, root)
    return packs, res
def install(undo=False):
    cs = CLAUDE_DIR / 'settings.json'; ch = CODEX_DIR / 'hooks.json'; snapshot([cs, ch], 'uninstall' if undo else 'install')
    def edit(f, host):
        d = jload(f, {}) or {}; h = d.setdefault('hooks', {})
        for ev in list(h):   # drop our entries and the old skill-router sync
            h[ev] = [g for g in h[ev] if not any(HOOK_MARK in x.get('command', '') or 'skill-router.py sync' in x.get('command', '') for x in g.get('hooks', []))]
            if not h[ev]: del h[ev]
        if not undo:
            mk = lambda e, tm=5: {'type': 'command', 'command': f'python -I {SCRIPT} hook {host} {e}', 'timeout': tm}
            h.setdefault('SessionStart', []).append({'hooks': [mk('session', 30)]})
            h.setdefault('UserPromptSubmit', []).append({'hooks': [mk('prompt')]})
            h.setdefault('PreToolUse', []).append({'matcher': 'mcp__.*|Skill|Bash|PowerShell' if host == 'claude' else 'mcp__.*|Bash', 'hooks': [mk('pretool')]})
        jsave(f, d)
    edit(cs, 'claude'); edit(ch, 'codex'); print('hooks', 'removed from' if undo else 'installed in', cs, 'and', ch)

# ---------- usage / audit (deterministic part for kritic) ----------
def usage_records(sid):
    p = STATE / 'usage' / (re.sub(r'[^\w-]', '_', sid or 'nosession') + '.jsonl')
    return [json.loads(l) for l in p.read_text(encoding='utf8').splitlines() if l.strip()] if p.exists() else []
def audit(sid, root):
    recs = usage_records(sid); plan = load_plan(sid, str(root)); called = {}
    for r in recs:
        if r.get('ev') == 'call': k = (r['kind'], r['name']); called[k] = called.get(k, 0) + 1
    denied = sorted({(r['kind'], r['name']) for r in recs if r.get('ev') == 'deny'})
    planned = sorted(plan['packs']) if plan else []
    pack_calls = {pk: sum(n for (k, nm), n in called.items() if pack_of({'skill': 'skills', 'mcp': 'mcp'}.get(k, 'x'), nm) == pk) for pk in planned}
    return dict(plan=plan, called={f'{k}:{n}': c for (k, n), c in called.items()}, denied=[f'{k}:{n}' for k, n in denied],
                unused_packs=[pk for pk, c in pack_calls.items() if c == 0], pack_calls=pack_calls)

# ---------- main ----------
def opt(a, flag, default=None):
    if flag in a: i = a.index(flag); v = a[i + 1] if i + 1 < len(a) else default; del a[i:i + 2]; return v
    return default
def main(a):
    cmd = a[0] if a else 'status'; a = a[1:]
    if cmd == 'hook': return hook(a[0], a[1])
    host = opt(a, '--host', 'claude'); proj = opt(a, '--project', os.getcwd()); sid = opt(a, '--session'); task = opt(a, '--task')
    preset = opt(a, '--preset'); pk = opt(a, '--pack'); core = '--core' in a; none = '--none' in a; full = '--full' in a; a = [x for x in a if x not in ('--none', '--full', '--core')]; root = root_of(proj)
    if cmd == 'catalog': print(catalog_text(root, host, a[0] if a else None, full))
    elif cmd == 'status':
        print('root', root, '| detected', detect(root), '| forced', {k: sorted(v) for k, v in forced(root).items()}, '| active', active_packs(root))
        p = load_plan(sid, str(root)); print('plan', p if p else 'none declared')
    elif cmd == 'plan':
        if not a and not none and not preset: sys.exit('plan needs PACK/name args, --preset NAME or --none')
        cmd_plan(a, sid, task, root, none, preset)
    elif cmd == 'register': cmd_register(a, pk, core, root)
    elif cmd == 'unregistered': print('\n'.join(unregistered(root)) or 'none')
    elif cmd == 'preset': cmd_preset(a, root)
    elif cmd == 'baseline':
        r = registry(); snapshot([STATE / 'registry.json'], 'baseline'); r['skills'] = sorted(set(r['skills']) | disk_skills()); jsave(STATE / 'registry.json', r); print('baseline:', len(r['skills']), 'skills registered')
    elif cmd in ('enable', 'disable'):
        on = cmd == 'enable'
        for name in a:
            if name.startswith('tool:'): toggle_one(name[5:], on, host, root)
            elif host == 'codex' and (name in codex_skills() or ('mcp', name) in codex_blocks() or ('plugin', name) in codex_blocks()): toggle_one(name, on, host, root)
            elif name.split(':')[0] in CFG['packs']:
                c = jload(root / '.claude/tool-router.json', {}) or {}; (root / '.claude').mkdir(exist_ok=True)
                snapshot([root / '.claude/tool-router.json', root / '.claude/skills/.skill-router.json'], 'pack')
                s_on, s_off = set(c.get('on', [])), set(c.get('off', []))
                if on: s_on.add(name); s_off.discard(name)
                else: s_on.discard(name); s_off.add(name)
                jsave(root / '.claude/tool-router.json', {'on': sorted(s_on), 'off': sorted(s_off)})
                packs, res = do_sync(root, host); print(f'pack {name}: {cmd}d for {root}; active={packs}; applies to NEXT session (plan applies live)')
                if not on and ':' not in name:
                    for n in claude_mcp_state(root):
                        if pack_of('mcp', n) == name and not gmatch(n, PROTECTED): mcp_disable(n, root)
            else: toggle_one(name, on, host, root)
    elif cmd == 'sync': packs, res = do_sync(root_of(a[0] if a else proj), host); print(f'packs={packs} {res}')
    elif cmd == 'usage': print(json.dumps(audit(sid or (a[0] if a else ''), root), indent=1))
    elif cmd == 'cli': cmd_cli(a)
    elif cmd == 'install': install()
    elif cmd == 'uninstall': install(True)
    elif cmd == 'restore': restore(a[0] if a else None)
    elif cmd == 'backups': print('\n'.join(sorted(p.name for p in (STATE / 'backups').glob('*'))))
    else: sys.exit(__doc__)
def toggle_one(name, on, host, root):
    if host == 'codex':
        cs = codex_skills(); cb = codex_blocks()
        if name in cs: codex_skill_set(name, on)
        elif ('mcp', name) in cb: codex_set('mcp', name, on)
        elif ('plugin', name) in cb: codex_set('plugin', name, on)
        else: sys.exit(f'codex: unknown {name}')
        return
    if name in plugins_state(): set_plugin(name, on)
    elif name in mcp_library() or name in claude_mcp_state(root): (mcp_enable if on else mcp_disable)(name, root)
    elif name in skills_state(root): print('skills toggle via packs (junction into project): enable/disable the pack that contains it')
    else: sys.exit(f'unknown tool {name}')
if __name__ == '__main__':
    try: main(sys.argv[1:])
    except SystemExit: raise
    except Exception as e: print('tool-router:', e, file=sys.stderr); sys.exit(0 if sys.argv[1:2] in (['hook'], ['sync']) else 1)

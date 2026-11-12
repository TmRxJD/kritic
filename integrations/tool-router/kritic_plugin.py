"""kritic plugin adapter for the tool router. stdin: {"args":{action,session,project,task,packs},"base_dir","out_dir"}; stdout: one JSON object.
Actions: catalog | plan | audit (deterministic findings from the usage log + Kev-triage state)."""
import importlib.util, io, json, sys, contextlib
from pathlib import Path
here = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('tool_router', here / 'tool-router.py'); tr = importlib.util.module_from_spec(spec); spec.loader.exec_module(tr)
req = json.loads(sys.stdin.read() or '{}'); a = req.get('args') or {}
root = tr.root_of(a.get('project') or req.get('base_dir') or '.'); sid = a.get('session', ''); action = a.get('action', 'audit')
def out(o): print(json.dumps(o))
if action == 'catalog': out({'ok': True, 'approval': True, 'summary': tr.catalog_text(root, a.get('host', 'claude'), a.get('pack'))}); sys.exit()
if action == 'plan':
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf): tr.cmd_plan(list(a.get('packs', [])), sid, a.get('task', ''), root, not a.get('packs'), a.get('preset'))
    out({'ok': True, 'approval': True, 'summary': buf.getvalue().strip()}); sys.exit()
au = tr.audit(sid, root); plan = au['plan']; F = []
def f(sev, msg): F.append({'severity': sev, 'message': msg})
if plan is None and au['called']:
    gated = [k for k in au['called'] if not tr.is_core_mcp(k.split(':', 1)[1])]
    if gated: f('high', f'gated tools called with no plan declared: {gated}')
if plan is None: f('medium', 'no tool plan was declared for this session')
for d in au['denied']: f('info', f'call denied by router (agent had to extend or declare plan): {d}')
for pk in au['unused_packs']: f('low', f'pack "{pk}" was in the plan but no tool from it was called: consider dropping it next time')
un = tr.unregistered(root)
if un: f('medium', f'unregistered skills present: {un} (run `register`)')
used = [k for k in au['called'] if not tr.is_core_mcp(k.split(':', 1)[1]) or k.split(':', 1)[0] == 'skill']
cat = {f"{i['kind']}:{i['name']}": i['desc'] for i in tr.build_catalog(root, 'claude')}
used_lines = '\n'.join(f"{k} x{au['called'][k]} - {cat.get(k, '')[:80]}" for k in used)[:1200] or 'none'
state = {'request': (plan or {}).get('task', '')[:600] or 'unknown', 'used': used_lines}
out({'ok': True, 'summary': f"plan={'yes' if plan else 'NO'} called={len(au['called'])} denied={len(au['denied'])} unused_packs={au['unused_packs']}",
     'findings': F, 'metrics': {'called': len(au['called']), 'denied': len(au['denied']), 'unused_packs': len(au['unused_packs'])}, 'kev_state': state})

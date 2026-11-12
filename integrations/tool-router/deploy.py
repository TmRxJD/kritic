"""Copy this directory to ~/.claude/skill-router (the path the hooks use). Keeps an existing tools.json. Run: python deploy.py [dest]"""
import shutil, sys
from pathlib import Path
src = Path(__file__).resolve().parent; dst = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.home() / '.claude/skill-router'
dst.mkdir(parents=True, exist_ok=True)
for f in ('tool-router.py', 'kritic_plugin.py', 'test_tool_router.py', 'README.md', 'deploy.py'): shutil.copy2(src / f, dst / f)
if not (dst / 'tools.json').exists(): shutil.copy2(src / 'tools.json', dst / 'tools.json')
(dst / '.kritic/plugins').mkdir(parents=True, exist_ok=True)
t = (src / '.kritic/plugins/tool-router.toml').read_text(encoding='utf8').replace('integrations/tool-router/kritic_plugin.py', (dst / 'kritic_plugin.py').as_posix())
(dst / '.kritic/plugins/tool-router.toml').write_text(t, encoding='utf8'); print('deployed to', dst)

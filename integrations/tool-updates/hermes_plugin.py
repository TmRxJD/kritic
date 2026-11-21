"""Optional Hermes adapter; refusing stale tools needs real hook block support."""
import json
from pathlib import Path
import subprocess  # nosec B404
import sys
import uuid


def register(ctx):
    from hermes_cli.plugins import get_pre_tool_call_directive
    marker='kritic-update-capability-'+uuid.uuid4().hex
    def probe(tool_name='',args=None,**kwargs):
        if tool_name=='list_tools' and (args or {}).get('_kritic_update_probe')==marker:
            return {'action':'block','message':marker}
        return None
    ctx.register_hook('pre_tool_call',probe)
    action,message=get_pre_tool_call_directive('list_tools',{'_kritic_update_probe':marker},session_id=marker)
    if action!='block' or message!=marker:
        raise RuntimeError('Actual Hermes dispatcher did not honor the update-blocking probe; gate was not silently enrolled')
    settings=json.loads((Path(__file__).parent/'updater.json').read_text())
    interpreter=Path(settings.get('python',sys.executable))
    if not interpreter.is_absolute() or not interpreter.is_file():
        raise ValueError('Configure the absolute enrolled updater Python interpreter')
    def pre_tool_call(tool_name='',args=None,task_id='',session_id='',**kwargs):
        session=session_id or task_id
        if not session: return {'action':'block','message':'Tool update gate needs actual Hermes session identity'}
        payload={'session_id':session,'tool_name':tool_name,'tool_input':dict(args or {})}
        try:
            result=subprocess.run([str(interpreter),'-I',settings['updater'],'--config',settings['config'],'hook','hermes','pretool'],
                input=json.dumps(payload),text=True,capture_output=True,timeout=45,
                creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0),check=False)  # nosec B603
            if result.returncode: raise ValueError('Updater process failed')
            return json.loads(result.stdout)
        except Exception as error:
            return {'action':'block','message':'Kritic update check unavailable: '+str(error)}
    ctx.register_hook('pre_tool_call',pre_tool_call)

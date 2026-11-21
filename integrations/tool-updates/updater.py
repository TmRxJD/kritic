"""Native operational control plane; no model dependency and no update shell scripts."""
import argparse
import json
import os
from pathlib import Path
import sys
import threading

sys.path.insert(0,str(Path(__file__).resolve().parent))
from config import DEFAULT_INTERVAL,load
import engine
from hook import control_plane,evaluate,repair_call,response
import state


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config')
    actions=parser.add_subparsers(dest='action',required=True)
    for name in ('schema','status','check','inventory'):
        action=actions.add_parser(name)
        action.add_argument('--tool')
        action.add_argument('--force',action='store_true')
    hook=actions.add_parser('hook'); hook.add_argument('host',choices=['codex','claude','hermes']); hook.add_argument('event',choices=['pretool','session'])
    worker=actions.add_parser('worker'); worker.add_argument('--tool',required=True); worker.add_argument('--lease',required=True); worker.add_argument('--operation',choices=['check','download'],required=True)
    args=parser.parse_args()
    payload=None
    try:
        if args.action=='hook':
            raw=sys.stdin.buffer.read(1_000_001)
            if len(raw)>1_000_000: raise ValueError('Hook payload exceeds1MB')
            payload=json.loads(raw)
            if not isinstance(payload,dict): raise ValueError('Hook payload must be an object')
            if control_plane(payload.get('tool_name',''),payload.get('tool_input') or payload.get('args') or {}):
                print('{}'); return 0
        if args.action=='schema':
            print(json.dumps({'version':1,'default_interval_seconds':DEFAULT_INTERVAL,'personal_daily_seconds':86400,
                'sources':['npm','pypi','github','blender'],'installed':['npm','python_metadata','command'],
                'configuration':str(Path(args.config or os.environ.get('KRITIC_UPDATES_CONFIG',Path.home()/'.kritic/updates.toml')).resolve()),'guide':'docs/tool-updates.md',
                'contract':'Only enrolled tool identities are enforced. Downloads are verified staging, not activation; loaded MCP processes require host reconnection.'}))
            return 0
        config=load(args.config)
        if args.action=='hook': output=evaluate(config,args.host,args.event,payload)
        elif args.action=='inventory':
            output={'managed_tools':[{'id':t['id'],'tool_prefixes':t['tool_prefixes'],'programs':t['programs'],'source':t['source']} for t in config['tool']],
                    'coverage':config.get('_coverage',[]),
                    'limits':'Enrolled/discovered prefixes and direct program calls only; opaque nested code, remote host tools and unknown aliases remain unverified.'}
        else:
            selected=[t for t in config['tool'] if not args.tool or t['id']==args.tool]
            if args.tool and not selected: raise ValueError('Unknown enrolled tool ID')
            if args.action=='worker':
                def expire():
                    try: engine.timeout_worker(config,selected[0],args.lease)
                    finally: os._exit(124)  # Terminate only this owned worker, never a tool/application.
                timer=threading.Timer(40 if args.operation=='check' else 330,expire)
                timer.daemon=True; timer.start()
                try: engine.worker(config,selected[0],args.lease,args.operation)
                finally: timer.cancel()
                output={'status':'checked' if args.operation=='check' else 'downloaded','active':False}
            elif args.action=='status':
                output={'enabled':config['policy']['enabled'],'interval_seconds':config['policy']['interval_seconds'],
                        'tools':[{'id':t['id'],'state':state.read(config['policy'],t['id'])} for t in selected]}
            else: output={'tools':[engine.check(config,t,force=args.force) for t in selected]}
        print(json.dumps(output))
        return 0
    except Exception as error:
        if args.action=='hook':
            if isinstance(payload,dict):
                policy_path=Path(args.config or os.environ.get('KRITIC_UPDATES_CONFIG',Path.home()/'.kritic/updates.toml')).resolve()
                tool=payload.get('tool_name',''); data=payload.get('tool_input') or payload.get('args') or {}
                if not isinstance(data,dict): data={}
                if repair_call(tool,data,{'_path':str(policy_path),'tool':[]}):
                    print('{}'); return 0
                if tool in ('Write','Edit','write_file','edit_file') and Path(data.get('file_path') or data.get('path') or '/missing').resolve()==policy_path:
                    print(json.dumps(response(args.host,'Repairing the exact update policy is allowed; preserve required update enforcement.'))); return 0
            print(json.dumps(response(args.host,'Kritic tool-update gate could not verify policy/freshness: '+str(error)+'. Use the exact installed updater schema/status command to repair configuration; never mark a failed check fresh.',True)))
            return 0
        print(json.dumps({'error':str(error)})); return 1


if __name__=='__main__':
    sys.exit(main())

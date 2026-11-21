"""Behavioral freshness, migration, integrity and cross-agent lifecycle regressions."""
import concurrent.futures
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

sys.path.insert(0,str(Path(__file__).resolve().parent))
import config
import engine
import hook
import providers
import staging
import state
import hermes_plugin


class UpdateTests(unittest.TestCase):
    def test_hermes_does_not_drop_once_per_session_notices(self):
        notice=hermes_plugin.host_directive({'context':'Recorded migration exception'})
        self.assertEqual(notice['action'],'block')
        self.assertIn('retry the unchanged tool call',notice['message'])
        self.assertEqual(hermes_plugin.host_directive({}),{})
        denied={'action':'block','message':'outdated'}
        self.assertEqual(hermes_plugin.host_directive(denied),denied)

    def setUp(self):
        parent=os.environ.get('UPDATE_TEST_ROOT',tempfile.gettempdir())
        self.root=Path(parent)/('tool-updates-'+str(time.time_ns()))
        self.root.mkdir(parents=True)
        self.manifest=self.root/'package.json'
        self.manifest.write_text(json.dumps({'name':'fixture-public','version':'1.0.0','bin':{'fixture':'server.js'}}))
        script=self.root/'server.js'; script.write_text('fixture')
        client=self.root/'registered.json'
        client.write_text(json.dumps({'mcpServers':{'fixture':{'command':sys.executable,'args':[str(script)]}}}))
        self.tool={'id':'fixture','tool_prefixes':['mcp__fixture__'],'programs':['fixture','rea.mjs'],
            'source':{'kind':'npm','package':'fixture-public'},'installed':{'kind':'npm','path':str(self.manifest),'client_bindings':[str(client)]},'compatibility':'same_major'}
        self.policy={'enabled':True,'interval_seconds':604800,'retry_seconds':600,'state_dir':str(self.root/'state'),
                     'auto_download':True,'max_download_bytes':1024**2}
        self.config={'version':1,'policy':self.policy,'tool':[self.tool],'_path':str(self.root/'updates.toml')}
        self.release={'latest':'1.0.0','source_url':'https://registry.npmjs.org/fixture-public/latest',
            'artifact':{'url':'https://registry.npmjs.org/fixture-public/-/fixture-public-1.0.0.tgz','algorithm':'sha256','digest':hashlib.sha256(b'payload').hexdigest(),'hosts':['registry.npmjs.org']},'migration_flags':{}}

    def check(self,now=1000,**kwargs): return engine.check(self.config,self.tool,clock=now,spawn=False,**kwargs)

    def test_weekly_default_daily_override_and_strict_config(self):
        path=self.root/'updates.toml'
        path.write_text('version=1\n[policy]\nstate_dir='+json.dumps(str(self.root/'cache'))+'\n')
        self.assertEqual(config.load(path)['policy']['interval_seconds'],604800)
        path.write_text(path.read_text()+'interval_seconds=86400\n')
        self.assertEqual(config.load(path)['policy']['interval_seconds'],86400)
        path.write_text(path.read_text()+'unknown=true\n')
        with self.assertRaises(ValueError): config.load(path)

    def test_fresh_cache_has_no_network_and_expiry_refreshes_once(self):
        with patch.object(providers,'metadata',return_value=self.release) as fetch:
            self.assertTrue(self.check()['network_checked'])
            for now in (1001,2000,604799+1000): self.assertFalse(self.check(now)['network_checked'])
            self.assertTrue(self.check(605800)['network_checked'])
        self.assertEqual(fetch.call_count,2)

    def test_daily_and_per_tool_interval_take_effect(self):
        self.policy['interval_seconds']=86400
        with patch.object(providers,'metadata',return_value=self.release) as fetch:
            self.check(); self.check(87399); self.check(87400)
            self.assertEqual(fetch.call_count,2)
            self.tool['interval_seconds']=300
            self.check(88000); self.check(88300)
            self.assertEqual(fetch.call_count,4)

    def test_failed_expired_check_does_not_extend_success_and_backoff_is_quiet(self):
        with patch.object(providers,'metadata',return_value=self.release): self.check()
        with patch.object(providers,'metadata',side_effect=ValueError('offline')) as fetch:
            self.assertFalse(self.check(700000)['allow'])
            self.assertFalse(self.check(700001)['allow'])
            self.assertEqual(fetch.call_count,1)
        self.assertEqual(state.read(self.policy,'fixture')['checked_at'],1000)

    def test_probe_cache_reuses_observation_and_invalidates_changed_metadata(self):
        real=providers.installed
        with patch.object(providers,'installed',wraps=real) as probe,patch.object(providers,'metadata',return_value=self.release):
            self.assertTrue(engine.check(self.config,self.tool,clock=1000)['allow'])
            self.assertTrue(engine.check(self.config,self.tool,clock=1001)['allow'])
            self.assertEqual(probe.call_count,1)
            manifest=json.loads(self.manifest.read_text()); manifest['version']='1.0.1'
            self.manifest.write_text(json.dumps(manifest))
            self.assertTrue(engine.check(self.config,self.tool,clock=1002)['allow'])
            self.assertEqual(probe.call_count,2)
            self.assertTrue(engine.check(self.config,self.tool,clock=700000)['allow'])
            self.assertEqual(probe.call_count,3)

    def test_future_timestamp_is_not_fresh(self):
        with patch.object(providers,'metadata',return_value=self.release) as fetch:
            self.check(9000); self.check(1000)
            self.assertEqual(fetch.call_count,2)

    def test_missing_enrolled_policy_is_not_silent_disable(self):
        with self.assertRaisesRegex(ValueError,'policy file is missing'):
            config.load(self.root/'missing.toml')
        with patch.dict(os.environ,{'KRITIC_UPDATES_CONFIG':str(self.root/'missing.toml')}):
            with self.assertRaisesRegex(ValueError,'policy file is missing'): config.load()

    def test_concurrent_agents_share_one_check_lease(self):
        started=threading.Event(); finish=threading.Event()
        def fetch(_): started.set(); finish.wait(3); return self.release
        with patch.object(providers,'metadata',side_effect=fetch) as request:
            with concurrent.futures.ThreadPoolExecutor(2) as pool:
                first=pool.submit(self.check)
                self.assertTrue(started.wait(2))
                second=pool.submit(self.check)
                self.assertEqual(second.result()['status'],'check_or_download_pending')
                finish.set(); self.assertTrue(first.result()['allow'])
            self.assertEqual(request.call_count,1)

    def test_due_hook_uses_owned_worker_and_never_networks_in_hook(self):
        payload={'session_id':'session','tool_name':'mcp__fixture__call','tool_input':{}}
        with patch.object(engine,'start_worker') as spawn,patch.object(providers,'metadata',side_effect=AssertionError('network in hook')):
            self.assertIn('check_pending',str(hook.evaluate(self.config,'codex','pretool',payload)))
            self.assertIn('pending',str(hook.evaluate(self.config,'codex','pretool',payload)))
            self.assertEqual(spawn.call_count,1)
            self.assertEqual(spawn.call_args.args[3],'check')

    def test_known_update_cannot_pass_before_download_or_activation(self):
        self.release['latest']='1.1.0'
        with patch.object(providers,'metadata',return_value=self.release):
            self.assertEqual(self.check()['status'],'download_required')
        receipt={'version':'1.1.0','path':str(self.root/'artifact'),'active':False}
        with state.transaction(self.policy) as db:
            db.execute('UPDATE tools SET artifact=? WHERE id=?',(json.dumps(receipt),'fixture'))
        self.assertEqual(self.check(1001)['status'],'activation_required')
        self.manifest.write_text(json.dumps({'name':'fixture-public','version':'1.1.0','bin':{'fixture':'server.js'}}))
        self.assertTrue(self.check(1002)['allow'])

    def test_migration_conflict_stops_download_and_exception_is_version_bound(self):
        self.release['latest']='2.0.0'
        with patch.object(providers,'metadata',return_value=self.release),patch.object(engine,'start_worker') as spawn:
            self.assertEqual(self.check()['status'],'migration_review_required')
            spawn.assert_not_called()
        self.tool['pin']={'version':'1.0.0','latest':'2.0.0','reason':'New API needs migration','evidence_url':'https://example.invalid/notes'}
        with patch.object(providers,'metadata',return_value=self.release): self.assertEqual(self.check(1001)['status'],'migration_exception')
        self.release['latest']='2.1.0'
        with patch.object(providers,'metadata',return_value=self.release):
            self.assertEqual(self.check(1002,force=True)['status'],'migration_exception_stale')

    def test_migration_notice_is_once_per_session_and_change(self):
        self.release['latest']='2.0.0'
        self.tool['pin']={'version':'1.0.0','latest':'2.0.0','reason':'Migration needed','evidence_url':'https://example.invalid/notes'}
        packet={'session_id':'session','tool_name':'mcp__fixture__call','tool_input':{}}
        with patch.object(providers,'metadata',return_value=self.release): engine.check(self.config,self.tool)
        self.assertIn('migration_exception',str(hook.evaluate(self.config,'claude','pretool',packet)))
        self.assertEqual(hook.evaluate(self.config,'claude','pretool',packet),{})

    def test_source_change_invalidates_cache_and_old_completion_cannot_rewind_it(self):
        def change_policy(_):
            newer=copy.deepcopy(self.tool); newer['source']['package']='replacement'
            with state.transaction(self.policy) as db: state.row(db,'fixture',config.fingerprint(newer))
            return self.release
        with patch.object(providers,'metadata',side_effect=change_policy):
            self.assertEqual(self.check()['status'],'policy_or_lease_changed')
        self.assertIsNone(state.read(self.policy,'fixture')['checked_at'])

    def test_no_downgrade_for_ahead_install(self):
        self.manifest.write_text(json.dumps({'name':'fixture-public','version':'1.2.0','bin':{'fixture':'server.js'}}))
        with patch.object(providers,'metadata',return_value=self.release):
            self.assertEqual(self.check()['status'],'ahead_of_stable')

    def test_runtime_and_zero_major_compatibility_are_conservative(self):
        self.assertFalse(providers.satisfies('22.20.0','^22.23.4 || ^24.22.1 || >=26.12.1'))
        self.assertTrue(providers.satisfies('22.23.5','^22.23.4 || ^24.22.1 || >=26.12.1'))
        self.assertTrue(providers.satisfies('3.13.1','>=3.10'))
        self.assertTrue(providers.satisfies('22.23.5','>=18'))
        self.assertTrue(providers.satisfies('22.23.5','^18 || ^20 || ^22'))
        self.assertTrue(providers.satisfies('3.13.1','~=3.10','pypi'))
        self.assertFalse(providers.satisfies('3.13.1','~=3.10.0','pypi'))
        self.assertFalse(providers.satisfies('0.2.0','^0.0.1'))
        with self.assertRaises(ValueError): providers.satisfies('3.13.1','!=3.12.*')
        current={'version':'0.2.0','runtime_version':None}
        self.assertIsNotNone(providers.compatibility(self.tool,current,dict(self.release,latest='0.3.0')))

    def test_release_notes_only_flag_unapplied_versions(self):
        release=dict(self.release,latest='1.2.0',notes_url='https://example.invalid/notes',notes_review=[{'version':'1.1.0','warnings':['breaking migration']}])
        self.assertIsNotNone(providers.compatibility(self.tool,{'version':'1.0.0'},release))
        self.assertIsNone(providers.compatibility(self.tool,{'version':'1.1.0'},release))

    def test_client_telemetry_does_not_look_like_a_binding_change(self):
        client=self.root/'client.json'
        script=self.root/'server.js'; script.write_text('fixture')
        server={'command':sys.executable,'args':[str(script)]}
        client.write_text(json.dumps({'mcpServers':{'fixture':server},'uses':1}))
        self.tool['installed']['client_bindings']=[str(client)]
        engine.session_start(self.config,'codex','session')
        with patch.object(providers,'metadata',return_value=self.release): self.check(host='codex',session='session')
        client.write_text(json.dumps({'mcpServers':{'fixture':server},'uses':99}))
        self.assertTrue(self.check(1001,host='codex',session='session')['allow'])
        server['env']={'MODE':'new'}
        client.write_text(json.dumps({'mcpServers':{'fixture':server},'uses':99}))
        self.assertEqual(self.check(1002,host='codex',session='session')['status'],'reconnect_required')

    def test_fresh_session_cannot_probe_new_install_but_launch_old_script(self):
        client=self.root/'client.json'
        old=self.root/'old/server.js'; old.parent.mkdir(); old.write_text('old')
        current=self.root/'current'; current.mkdir()
        manifest=current/'package.json'; manifest.write_text(self.manifest.read_text())
        (current/'server.js').write_text('current')
        self.tool['installed']['path']=str(manifest)
        self.tool['installed']['client_bindings']=[str(client)]
        self.tool['installed']['runtime']=sys.executable
        client.write_text(json.dumps({'mcpServers':{'fixture':{'command':sys.executable,'args':[str(old)]}}}))
        engine.session_start(self.config,'codex','fresh')
        with patch.object(providers,'command',return_value='22.23.5'),patch.object(providers,'metadata',return_value=self.release):
            result=self.check(host='codex',session='fresh')
        self.assertFalse(result['allow'])
        self.assertIn('canonical',result['error'])

    def test_managed_mcp_control_plane_leaf_names_do_not_bypass_check(self):
        with patch.object(engine,'start_worker') as start:
            for leaf in ('read_file','send_message','worktree_evaluate','cli_run'):
                output=hook.evaluate(self.config,'codex','pretool',{'session_id':'s','tool_name':'mcp__fixture__'+leaf,'tool_input':{'action':'status'}})
                self.assertIn('deny',str(output))
            self.assertEqual(start.call_count,1)

    def test_native_windows_backslashes_are_preserved(self):
        self.assertEqual(hook.command_tokens(r'D:\Tools\blender.exe --version',windows=True),[r'D:\Tools\blender.exe','--version'])
        self.assertEqual(hook.command_tokens(r'& "D:\Tool Folder\blender.exe" --version',windows=True),[r'D:\Tool Folder\blender.exe','--version'])

    def test_custom_policy_repair_guidance_uses_exact_policy_path(self):
        with patch.object(engine,'start_worker'):
            response=hook.evaluate(self.config,'claude','pretool',{'session_id':'s','tool_name':'mcp__fixture__call','tool_input':{}})
        self.assertIn('--config',str(response))
        self.assertIn(self.config['_path'].replace('\\','\\\\'),json.dumps(response))

    def test_discovery_enrolls_node_package_and_reports_unknown_local_server(self):
        import inventory
        package=self.root/'node_modules/fixture-public'; package.mkdir(parents=True)
        (package/'package.json').write_text(self.manifest.read_text())
        script=package/'server.js'; script.write_text('fixture')
        client=self.root/'client.json'
        client.write_text(json.dumps({'mcpServers':{'node-fixture':{'command':sys.executable,'args':[str(script)]},'custom':{'command':str(self.root/'unknown.exe')}}}))
        # A real Node filename is the discovery discriminator; no process is run.
        node=self.root/('node.exe' if os.name=='nt' else 'node'); node.write_text('fixture')
        value=json.loads(client.read_text()); value['mcpServers']['node-fixture']['command']=str(node); client.write_text(json.dumps(value))
        tools,coverage=inventory.discover([str(client)],[],[])
        self.assertEqual(tools[0]['source']['package'],'fixture-public')
        self.assertTrue(any(item['status']=='local_unmanaged' for item in coverage))
        self.assertNotIn('env',str(coverage))

    def test_deployment_is_additive_idempotent_and_keeps_unrelated_updaters(self):
        import deploy
        policy=self.root/'updates.toml'
        policy.write_text('version=1\n[policy]\nstate_dir='+json.dumps(str(self.root/'install-state'))+'\n')
        home=self.root/'codex'; home.mkdir()
        configfile=home/'hooks.json'
        unrelated='python -I B:/other-tool/updater.py hook codex pretool'
        recovery='python -I B:/recovery/coordinator.py hook codex pretool'
        original={'hooks':{'PreToolUse':[{'hooks':[{'type':'command','command':unrelated},{'type':'command','command':recovery}]}]},'keep':{'x':1}}
        configfile.write_text(json.dumps(original))
        runtime=self.root/'runtime'
        deploy.install(runtime,policy,codex=home)
        first=configfile.read_bytes()
        deploy.install(runtime,policy,codex=home)
        self.assertEqual(first,configfile.read_bytes())
        data=json.loads(first)
        commands=[h['command'] for group in data['hooks']['PreToolUse'] for h in group['hooks']]
        self.assertIn(unrelated,commands); self.assertIn(recovery,commands)
        self.assertEqual(data['keep'],original['keep'])

    def test_config_error_leaves_schema_and_coordination_recovery_reachable(self):
        import subprocess  # nosec B404 - exercise the actual trusted updater CLI
        bad=self.root/'bad.toml'; bad.write_text('invalid = [')
        updater=Path(__file__).parent/'updater.py'
        flags=getattr(subprocess,'CREATE_NO_WINDOW',0)
        schema=subprocess.run([sys.executable,'-I',str(updater),'--config',str(bad),'schema'],capture_output=True,creationflags=flags,timeout=10)  # nosec B603 - fixed interpreter/script and owned fixture
        self.assertEqual(schema.returncode,0,schema.stderr)
        packet={'session_id':'s','tool_name':'mcp__kev_tools__worktree_evaluate','tool_input':{'action':'recover'}}
        child=subprocess.run([sys.executable,'-I',str(updater),'--config',str(bad),'hook','codex','pretool'],input=json.dumps(packet).encode(),capture_output=True,creationflags=flags,timeout=10)  # nosec B603 - fixed interpreter/script, no shell
        self.assertEqual(json.loads(child.stdout),{})

    def test_old_explicit_cli_path_cannot_hide_behind_current_canonical_version(self):
        self.tool['installed']={'kind':'command','argv':[sys.executable,'--version'],'client_bindings':[]}
        self.assertIsNotNone(hook.invocation_conflict('cli_run',{'program':str(self.root/'old-python.exe'),'args':[]},self.tool))
        self.assertIsNone(hook.invocation_conflict('cli_run',{'program':sys.executable,'args':[]},self.tool))

    def test_control_plane_and_exact_repair_command_remain_reachable(self):
        payload={'session_id':'s','tool_name':'mcp__kev_tools__worktree_evaluate','tool_input':{'action':'recover'}}
        self.assertEqual(hook.evaluate(self.config,'codex','pretool',payload),{})
        command={'program':'python','args':['-I',str(Path(__file__).parent/'updater.py'),'status']}
        self.assertTrue(hook.repair_call('cli_run',command,self.config))
        command['args']+=['&&','bad']
        self.assertFalse(hook.repair_call('cli_run',command,self.config))

    def test_integrity_failure_never_produces_verified_artifact(self):
        class Response(io.BytesIO): headers={}
        class Opener:
            def open(self,*args,**kwargs): return Response(b'corrupt')
        with patch('urllib.request.build_opener',return_value=Opener()):
            with self.assertRaisesRegex(ValueError,'integrity'): staging.download(self.policy,self.tool,self.release,'lease')
        self.assertFalse(list((self.root/'state').rglob('receipt.json')))

    def test_verified_staging_is_downloaded_not_active(self):
        class Response(io.BytesIO): headers={}
        class Opener:
            def open(self,*args,**kwargs): return Response(b'payload')
        with patch('urllib.request.build_opener',return_value=Opener()):
            receipt=staging.download(self.policy,self.tool,self.release,'lease')
        self.assertFalse(receipt['active'])
        self.assertEqual(Path(receipt['path']).read_bytes(),b'payload')
        with self.assertRaises(FileExistsError): staging.download(self.policy,self.tool,self.release,'lease')

    def test_unapproved_url_and_prerelease_versions_are_rejected(self):
        for url in ('http://registry.npmjs.org/x','https://evil.invalid/x','https://user:pass@registry.npmjs.org/x'):
            with self.assertRaises(ValueError): providers.validate_url(url,['registry.npmjs.org'])
        for release in ('1.0.0-beta','latest','v1.2','1.0.0/../../escape'):
            with self.assertRaises(ValueError): providers.version(release)


if __name__=='__main__': unittest.main()

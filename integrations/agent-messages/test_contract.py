"""Real validation/adapter controls: invalid content cannot be silently accepted."""
import json
import os
from pathlib import Path
import subprocess  # nosec B404
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from contract import schema, validate
from hook import response, violation
from deploy import install
import hermes_plugin


class MessageTests(unittest.TestCase):
    def task(self):
        return {'kind': 'task', 'goal': 'Fix the import boundary', 'scope': ['src/imports/'], 'done_when': ['Malformed imports are rejected'], 'constraints': ['Preserve other agents work']}

    def test_task_preserves_exact_identifiers(self):
        task = self.task()
        task['context'] = ['B:/AI/please-keep/thank-you.json:12', 'sha256:abc123']
        raw = json.dumps(task)
        self.assertEqual(validate(raw), task)
        self.assertIsNone(violation('collaboration.spawn_agent', {'message': raw}))

    def test_missing_scope_completion_and_unknown_fields_rejected(self):
        for field in ('scope', 'done_when'):
            value = self.task()
            del value[field]
            with self.assertRaises(ValueError):
                validate(json.dumps(value))
        value = self.task()
        value['pleasantry'] = 'thank you'
        with self.assertRaises(ValueError):
            validate(json.dumps(value))

    def test_plaintext_duplicate_fields_and_politeness_rejected(self):
        for raw in ('Please do the task, thanks!', '{"kind":"update","kind":"question","summary":"x"}', json.dumps({'kind':'update','summary':'Please review it'}), json.dumps({'kind':'update','summary':'Done. Thank you.'})):
            with self.assertRaises(ValueError):
                validate(raw)
        value = self.task()
        value['scope'] = ['TBD']
        with self.assertRaisesRegex(ValueError,'placeholders'):
            validate(json.dumps(value))

    def test_result_requires_evidence_and_explicit_unrun_reason(self):
        value = {'kind': 'result', 'summary': 'Import boundary updated', 'changes': ['src/imports/parser.rs'], 'checks': [{'name':'cargo test','status':'not_run'}], 'evidence': ['src/imports/parser.rs:42']}
        with self.assertRaisesRegex(ValueError, 'detail'):
            validate(json.dumps(value))
        value['checks'][0]['detail'] = 'Runner unavailable; not executed'
        self.assertEqual(validate(json.dumps(value)), value)
        value['evidence'] = []
        with self.assertRaises(ValueError):
            validate(json.dumps(value))

    def test_size_and_word_limits_fail_instead_of_truncating(self):
        with self.assertRaises(ValueError):
            validate('x' * 4097)
        with self.assertRaisesRegex(ValueError, 'words'):
            validate(json.dumps({'kind':'update','summary':'a ' * 100}))

    def test_task_creation_rejects_update(self):
        raw = json.dumps({'kind':'update','summary':'Working on imports'})
        self.assertIsNotNone(violation('spawn_agent', {'message':raw}))
        self.assertIsNone(violation('send_message', {'message':raw}))

    def test_all_harness_fields_and_nonmessage_tools(self):
        raw = json.dumps(self.task())
        for tool, args in [('mcp__codex_app__send_message_to_thread', {'prompt':raw}), ('Task', {'prompt':raw}), ('SendMessage', {'type':'message','content':raw}), ('message_agent', {'message':raw}), ('delegate_task', {'tasks':[{'goal':raw}]}), ('delegate_task', {'action':'steer','message':raw})]:
            self.assertIsNone(violation(tool,args), tool)
        self.assertIsNotNone(violation('delegate_task', {'tasks':[{'goal':raw,'context':'unchecked prose'}]}))
        self.assertIsNotNone(violation('delegate_task', {'tasks':[{'goal':raw},{'goal':'unchecked prose'}]}))
        self.assertIsNone(violation('delegate_task', {'action':'list'}))
        self.assertIsNone(violation('Edit', {'file_path':'src/a.rs','content':'user-facing thank you text'}))
        self.assertIsNone(violation('mcp__codex_app__fork_thread', {'threadId':'existing-thread','environment':'worktree'}))

    def test_native_hook_process_accepts_valid_and_denies_invalid(self):
        script = Path(__file__).resolve().parent / 'hook.py'
        for host in ('codex','claude','hermes'):
            for raw, denied in [(json.dumps(self.task()),False),('Please fix it, thanks',True)]:
                event = {'tool_name':'send_message','tool_input':{'message':raw}}
                child = subprocess.run([sys.executable,'-I',str(script),host], input=json.dumps(event), text=True,capture_output=True,check=True)  # nosec B603
                result = json.loads(child.stdout)
                expected = response(host, 'invalid')
                self.assertEqual(bool(result), bool(expected) if denied else False)
                if denied:
                    self.assertIn('contract',json.dumps(result))

    def test_all_five_kinds_have_required_fields(self):
        variants = schema()['oneOf']
        self.assertEqual(len(variants),5)
        for spec in variants:
            self.assertFalse(spec['additionalProperties'])
            self.assertIn('kind',spec['required'])

    def test_blocker_question_update_shapes(self):
        for value in [{'kind':'blocker','problem':'Import format ambiguous','evidence':['schema.json:12'],'needs':['Identify canonical version']},
                      {'kind':'question','question':'Which schema revision is canonical?','options':['v1','v2']},
                      {'kind':'update','summary':'Import tests running','next':'Report actual results'}]:
            self.assertEqual(validate(json.dumps(value)),value)

    def test_installer_preserves_existing_hooks_and_is_idempotent(self):
        root = Path(os.environ.get('AGENT_MESSAGE_TEST_ROOT',tempfile.gettempdir())) / f'agent-messages-{time.time_ns()}'
        codex,claude,runtime = root/'codex',root/'claude',root/'runtime'
        codex.mkdir(parents=True)
        claude.mkdir()
        existing = {'hooks':{'PreToolUse':[{'hooks':[{'type':'command','command':'existing-hook'}]}]},'permissions':{'allow':['Read']}}
        (codex/'hooks.json').write_text(json.dumps(existing),encoding='utf8')
        (claude/'settings.json').write_text(json.dumps(existing),encoding='utf8')
        install(runtime,codex,claude)
        install(runtime,codex,claude)
        for path in (codex/'hooks.json',claude/'settings.json'):
            actual = json.loads(path.read_text(encoding='utf8'))
            self.assertEqual(actual['permissions'],existing['permissions'])
            self.assertEqual(len(actual['hooks']['PreToolUse']),2)
            self.assertEqual(actual['hooks']['PreToolUse'][0],existing['hooks']['PreToolUse'][0])

    def test_hermes_register_hook_adapter_blocks_invalid_payload(self):
        root = Path(os.environ.get('AGENT_MESSAGE_TEST_ROOT',tempfile.gettempdir())) / f'message-plugin-{time.time_ns()}'
        root.mkdir(parents=True)
        script = Path(__file__).resolve().parent / 'hook.py'
        (root/'contract.json').write_text(json.dumps({'hook':str(script)}),encoding='utf8')
        callbacks = {}
        class NativeContext:
            def register_hook(self,name,callback):
                callbacks[name] = callback
        with patch.object(hermes_plugin,'_HERE',root):
            hermes_plugin.register(NativeContext())
        callback = callbacks['pre_tool_call']
        self.assertEqual(callback('message_agent',{'message':'Please review, thanks'})['action'],'block')
        self.assertIsNone(callback('message_agent',{'message':json.dumps(self.task())}))

    def test_installer_migrates_exact_managed_version_and_restricted_matcher(self):
        root = Path(os.environ.get('AGENT_MESSAGE_TEST_ROOT',tempfile.gettempdir())) / f'message-upgrade-{time.time_ns()}'
        codex,claude,old,new = root/'codex',root/'claude',root/'old',root/'new'
        install(old,codex,claude)
        path = codex/'hooks.json'
        data = json.loads(path.read_text(encoding='utf8'))
        data['hooks']['PreToolUse'][0]['matcher'] = 'Read'
        data['hooks']['PreToolUse'].append({'hooks':[{'type':'command','command':'unrelated-hook'}]})
        path.write_text(json.dumps(data),encoding='utf8')
        install(old,codex,claude)
        data = json.loads(path.read_text(encoding='utf8'))
        self.assertEqual(len(data['hooks']['PreToolUse']),2)
        self.assertNotIn('matcher',data['hooks']['PreToolUse'][-1])
        with self.assertRaisesRegex(ValueError,'replace-runtime'):
            install(new,codex,claude)
        install(new,codex,claude,replace_runtime=old)
        data = json.loads(path.read_text(encoding='utf8'))
        commands = [item['command'] for group in data['hooks']['PreToolUse'] for item in group['hooks']]
        self.assertIn('unrelated-hook',commands)
        self.assertEqual(sum(str(new/'hook.py').replace('\\','/') in command for command in commands),1)
        self.assertFalse(any(str(old/'hook.py').replace('\\','/') in command for command in commands))


if __name__ == '__main__':
    unittest.main()

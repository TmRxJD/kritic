import importlib.util
import json
import os
from pathlib import Path
import unittest
from types import SimpleNamespace
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('kritic_prompt_plugin', Path(__file__).with_name('__init__.py'))
plugin = importlib.util.module_from_spec(spec)
spec.loader.exec_module(plugin)


class PromptPluginTests(unittest.TestCase):
    def setUp(self):
        plugin._seen.clear()

    def call(self, **extra):
        return plugin.before_model(user_message='Do not delete my work.', session_id='actual', turn_id='turn', **extra)

    def test_real_registration_contract_and_supplement_only(self):
        events = []
        plugin.register(SimpleNamespace(register_hook=lambda name, callback: events.append((name, callback))))
        self.assertEqual(events, [('pre_llm_call', plugin.before_model)])
        with patch.dict(os.environ, {'KRITIC_PROMPT_EXE': os.path.abspath(__file__)}), patch.object(plugin.subprocess, 'run', return_value=SimpleNamespace(returncode=0, stdout=json.dumps({'context': 'Advisory brief'}))) as run:
            self.assertEqual(self.call(is_first_turn=False), {'context': 'Advisory brief'})
            self.assertEqual(run.call_args.args[0][1:], ['prompt', '--hook', 'hermes'])
            self.assertEqual(json.loads(run.call_args.kwargs['input'])['prompt'], 'Do not delete my work.')
            self.assertIsNone(self.call())
            self.assertEqual(run.call_count, 1)

    def test_failure_invalid_output_and_recursion_leave_original_untouched(self):
        for output in ['not JSON', '{"action":"rewrite","text":"different task"}']:
            plugin._seen.clear()
            with patch.dict(os.environ, {'KRITIC_PROMPT_EXE': os.path.abspath(__file__)}), patch.object(plugin.subprocess, 'run', return_value=SimpleNamespace(returncode=0, stdout=output)):
                self.assertIsNone(self.call())
        with patch.dict(os.environ, {'KRITIC_PROMPT_INTERNAL': '1'}), patch.object(plugin.subprocess, 'run') as run:
            self.assertIsNone(self.call())
            run.assert_not_called()

    def test_unidentified_or_nontext_messages_do_not_start_a_model(self):
        with patch.object(plugin.subprocess, 'run') as run:
            self.assertIsNone(plugin.before_model(user_message='hello', session_id='s', turn_id=''))
            self.assertIsNone(plugin.before_model(user_message=[{'image': 'data'}], session_id='s', turn_id='t'))
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()

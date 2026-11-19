"""Real Git worktree and concurrent reservation regressions; no user checkout mutations."""
import concurrent.futures
import json
import os
import shlex
import subprocess  # nosec B404
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from coordinator import request
from evaluate import evaluate, inspect
from hooks import hook
from paths import git, identity, overlaps
from registry import actor_id, declare, release, status


class CoordinationTests(unittest.TestCase):
    def setUp(self):
        parent = os.environ.get('COORDINATION_TEST_ROOT', tempfile.gettempdir())
        self.root = Path(parent) / f'coordination-{time.time_ns()}'
        self.root.mkdir(parents=True)
        self.env = patch.dict(os.environ, {'KRITIC_COORDINATION_DIR': str(self.root / 'state')})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.main = self.root / 'repo with [brackets]'
        self.main.mkdir()
        git(self.main, 'init', '-q', '-b', 'main')
        git(self.main, 'config', 'user.name', 'Coordination Fixture')
        git(self.main, 'config', 'user.email', 'fixture@example.invalid')
        git(self.main, 'config', 'core.hooksPath', str(self.root / 'no-hooks'))
        (self.main / 'src').mkdir()
        for name in ('a.txt', 'b.txt'):
            (self.main / 'src' / name).write_text('baseline\n', encoding='utf8')
        git(self.main, 'add', '--', 'src')
        git(self.main, 'commit', '-qm', 'baseline')
        self.a = self.root / 'tree a & one'
        self.b = self.root / 'tree b'
        git(self.main, 'worktree', 'add', '-q', '-b', 'feature-a', str(self.a))
        git(self.main, 'worktree', 'add', '-q', '-b', 'feature-b', str(self.b))

    def declaration(self, session, tree, scope, host='codex', agent=None):
        value = {'host': host, 'session_id': session, 'worktree': str(tree), 'task': 'Implement the assigned feature',
                 'change_summary': 'Modify the assigned file and preserve the other feature.',
                 'scopes': [{'path': scope, 'purpose': 'Implement the assigned feature in this location'}]}
        if agent:
            value['agent_id'] = agent
        return value

    def event(self, session, tree, tool='Edit', **extra):
        return dict(session_id=session, cwd=str(tree), tool_name=tool,
                    tool_input={'file_path': str(tree / 'src/a.txt')}, **extra)

    def commit(self, tree, file, text):
        (tree / file).write_text(text, encoding='utf8')
        git(tree, 'add', '--', file)
        git(tree, 'commit', '-qm', 'fixture change')

    def include_reviews(self, snapshot):
        result = []
        for tree in snapshot['worktrees']:
            for change in tree['changes']:
                for hunk in change['hunks']:
                    result.append({'worktree': tree['worktree'], 'head': tree['head'], 'owner_actor_id': tree['owner']['id'],
                                   'owner_generation': tree['owner']['generation'], 'hunk_id': hunk['id'], 'decision': 'include',
                                   'reason': 'Reviewed the actual change against the assigned feature intent'})
        return result

    def test_scope_overlap_is_component_based(self):
        self.assertTrue(overlaps('src/foo/', 'src/foo/a.txt'))
        self.assertFalse(overlaps('src/foo/', 'src/foobar/a.txt'))

    def test_missing_mcp_tool_can_discover_message_and_declare_with_native_command(self):
        hook('codex', 'session', self.event('a', self.a))
        hook('claude', 'session', self.event('b', self.b))
        for tool in ('tool_search.search', 'functions.search_tools', 'mcp__codex_app__send_message_to_thread', 'collaboration.send_message'):
            event = self.event('a', self.a, tool=tool)
            event['tool_input'] = {'query': 'worktree_evaluate'}
            self.assertEqual(hook('codex', 'pretool', event), {})
        denied = hook('codex', 'pretool', self.event('a', self.a))
        reason = denied['hookSpecificOutput']['permissionDecisionReason']
        self.assertIn('--request-json', reason)
        self.assertIn('session_id=a', reason)
        coordinator = Path(__file__).resolve().parent / 'coordinator.py'
        declaration = {'action': 'declare', 'request': self.declaration('a', self.a, 'src/a.txt')}
        command = f'python -I "{coordinator.as_posix()}" --request-json ' + shlex.quote(json.dumps(declaration))
        event = self.event('a', self.a, tool='exec_command')
        event['tool_input'] = {'cmd': command, 'workdir': str(self.a)}
        self.assertEqual(hook('codex', 'pretool', event), {})
        result = subprocess.run([sys.executable, '-I', str(coordinator), '--request-json', json.dumps(declaration)],
                                capture_output=True, text=True, check=True)  # nosec B603
        self.assertIn('declared', result.stdout)
        event['tool_input']['cmd'] = command + ' && echo unauthorized'
        self.assertEqual(hook('codex', 'pretool', event)['hookSpecificOutput']['permissionDecision'], 'deny')
        declaration['request']['session_id'] = 'b'
        event['tool_input']['cmd'] = f'python -I "{coordinator.as_posix()}" --request-json ' + shlex.quote(json.dumps(declaration))
        with self.assertRaisesRegex(ValueError, 'actual host/session/agent'):
            hook('codex', 'pretool', event)
        declaration['request']['session_id'] = 'a'
        declaration['request']['agent_id'] = 'child'
        event['tool_input']['cmd'] = f'python -I "{coordinator.as_posix()}" --request-json ' + shlex.quote(json.dumps(declaration))
        self.assertEqual(hook('codex', 'pretool', event), {})
        event['agent_id'] = 'different-child'
        with self.assertRaisesRegex(ValueError, 'actual host/session/agent'):
            hook('codex', 'pretool', event)

    def test_file_directory_aliases_conflict(self):
        self.assertTrue(overlaps('src/foo', 'src/foo/'))
        self.assertTrue(overlaps('src/foo', 'src/foo/new.txt'))

    def test_mixed_absolute_relative_patch_uses_actual_execution_cwd(self):
        declare(self.declaration('a', self.a, 'src/'))
        declare(self.declaration('b', self.b, 'README.md', host='claude'))
        event = self.event('a', self.a, tool='apply_patch')
        event['tool_input'] = {'command': f'*** Begin Patch\n*** Update File: {(self.a / "src/a.txt").as_posix()}\n@@\n-old\n+new\n*** Update File: b.txt\n@@\n-old\n+new\n*** End Patch'}
        self.assertEqual(hook('codex', 'pretool', event)['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_read_only_declaration_does_not_stall_parent_editor(self):
        hook('codex', 'session', self.event('family', self.a))
        hook('codex', 'subagent', self.event('family', self.a, agent_id='reader'))
        declare(self.declaration('family', self.a, 'src/a.txt'))
        reader = self.declaration('family', self.a, 'src/', agent='reader')
        reader['access'] = 'read_only'
        declare(reader)
        self.assertEqual(hook('codex', 'pretool', self.event('family', self.a)), {})
        attempted = self.event('family', self.a, agent_id='reader')
        self.assertEqual(hook('codex', 'pretool', attempted)['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_external_first_target_cannot_bypass_second_repository_target(self):
        declare(self.declaration('a', self.a, 'src/a.txt'))
        declare(self.declaration('b', self.b, 'README.md', host='claude'))
        event = self.event('a', self.a, tool='apply_patch')
        external = self.root / 'external.txt'
        event['tool_input'] = {'command': f'*** Begin Patch\n*** Add File: {external.as_posix()}\n+outside\n*** Update File: {(self.a / "src/b.txt").as_posix()}\n@@\n-old\n+new\n*** End Patch'}
        self.assertEqual(hook('codex', 'pretool', event)['hookSpecificOutput']['permissionDecision'], 'deny')
        self.assertFalse(external.exists())

    def test_active_owner_supersedes_closed_owner_but_old_review_is_rejected(self):
        declare(self.declaration('old-owner', self.a, 'src/a.txt'))
        self.commit(self.a, 'src/a.txt', 'feature\n')
        params = {'worktree': str(self.main), 'main_ref': 'refs/heads/main', 'worktrees': [str(self.a)]}
        before = inspect(params)
        tree = before['worktrees'][0]
        hunk = tree['changes'][0]['hunks'][0]
        review = {'worktree': str(self.a), 'head': tree['head'], 'owner_actor_id': tree['owner']['id'],
                  'owner_generation': tree['owner']['generation'], 'hunk_id': hunk['id'], 'decision': 'include', 'reason': 'Assigned feature'}
        release({'host': 'codex', 'session_id': 'old-owner', 'reason': 'Assignment complete'})
        declare(self.declaration('new-owner', self.a, 'src/a.txt'))
        current = inspect(params)
        self.assertEqual(current['worktrees'][0]['owner']['session'], 'new-owner')
        with self.assertRaisesRegex(ValueError, 'Ownership intent changed'):
            evaluate({'snapshot_request': params, 'main_revision': before['main_revision'], 'reviews': [review]})

    def test_claim_change_during_inspection_invalidates_snapshot(self):
        import evaluate as implementation
        declaration = self.declaration('a', self.a, 'src/a.txt')
        declare(declaration)
        self.commit(self.a, 'src/a.txt', 'feature\n')
        original = implementation.entry
        changed = False
        def mutate_once(*args):
            nonlocal changed
            if not changed:
                changed = True
                next_declaration = dict(declaration, change_summary='A newly assigned task intent replaces the prior intent.')
                declare(next_declaration)
            return original(*args)
        with patch.object(implementation, 'entry', side_effect=mutate_once):
            with self.assertRaisesRegex(ValueError, 'Ownership or task intent changed'):
                inspect({'worktree': str(self.main), 'main_ref': 'refs/heads/main', 'worktrees': [str(self.a)]})

    def test_overlapping_scope_and_shared_worktree_are_rejected(self):
        declare(self.declaration('a', self.a, 'src/'))
        with self.assertRaisesRegex(ValueError, 'Scope overlaps'):
            declare(self.declaration('b', self.b, 'src/a.txt', 'claude'))
        with self.assertRaisesRegex(ValueError, 'Worktree reserved'):
            declare(self.declaration('b', self.a, 'README.md', 'hermes'))

    def test_reservations_are_atomic_under_concurrent_declarations(self):
        values = [self.declaration('a', self.a, 'src/a.txt'), self.declaration('b', self.b, 'src/a.txt', 'hermes')]
        def reserve(value):
            try:
                declare(value)
                return 'declared'
            except ValueError:
                return 'conflict'
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(pool.map(reserve, values))
        self.assertEqual(sorted(outcomes), ['conflict', 'declared'])

    def test_multiple_harnesses_require_all_declarations_and_check_targets(self):
        hook('codex', 'session', self.event('a', self.a))
        hook('claude', 'session', self.event('b', self.b))
        declare(self.declaration('a', self.a, 'src/a.txt'))
        self.assertEqual(hook('codex', 'pretool', self.event('a', self.a))['hookSpecificOutput']['permissionDecision'], 'deny')
        declare(self.declaration('b', self.b, 'src/b.txt', 'claude'))
        self.assertEqual(hook('codex', 'pretool', self.event('a', self.a)), {})
        wrong = self.event('a', self.a)
        wrong['tool_input']['file_path'] = str(self.a / 'src/b.txt')
        self.assertEqual(hook('codex', 'pretool', wrong)['hookSpecificOutput']['permissionDecision'], 'deny')
        wrong['tool_input']['file_path'] = '../outside.txt'
        self.assertEqual(hook('codex', 'pretool', wrong)['hookSpecificOutput']['permissionDecision'], 'deny')

    def test_codex_shared_session_subagents_use_separate_declared_roots(self):
        hook('codex', 'session', self.event('family', self.a))
        hook('codex', 'subagent', self.event('family', self.a, agent_id='child'))
        declare(self.declaration('family', self.a, 'src/a.txt'))
        declare(self.declaration('family', self.b, 'src/b.txt', agent='child'))
        child = self.event('family', self.a, tool='apply_patch')
        child['tool_input'] = {'command': f'*** Begin Patch\n*** Update File: {(self.b / "src/b.txt").as_posix()}\n@@\n-baseline\n+new\n*** End Patch'}
        self.assertEqual(hook('codex', 'pretool', child), {})
        rows = status({'worktree': str(self.a)})['actors']
        self.assertEqual(len(rows), 2)
        self.assertTrue(all(row['declaration'] for row in rows))
        hook('codex', 'subagent_end', self.event('family', self.a, agent_id='child'))
        self.assertEqual(len(status({'worktree': str(self.a)})['actors']), 1)

    def test_hermes_block_directive_and_session_end_preserve_files(self):
        hook('hermes', 'session', self.event('a', self.a))
        hook('claude', 'session', self.event('b', self.b))
        self.assertEqual(hook('hermes', 'pretool', self.event('a', self.a))['action'], 'block')
        hook('hermes', 'end', {'session_id': 'a', 'cwd': str(self.root)})
        self.assertEqual((self.a / 'src/a.txt').read_text(), 'baseline\n')
        self.assertEqual(len(status({'worktree': str(self.b)})['actors']), 1)

    def test_release_preserves_dirty_and_untracked_work(self):
        receipt = declare(self.declaration('a', self.a, 'src/a.txt'))
        (self.a / 'src/a.txt').write_text('unfinished work\n')
        (self.a / 'untracked.txt').write_text('preserve me\n')
        release({'host': 'codex', 'session_id': 'a', 'reason': 'Task intentionally paused', 'expected_generation': receipt['generation']})
        self.assertEqual((self.a / 'untracked.txt').read_text(), 'preserve me\n')
        self.assertEqual((self.a / 'src/a.txt').read_text(), 'unfinished work\n')

    def test_schema_and_invalid_declarations_are_actionable(self):
        self.assertIn('declaration', request({'action': 'schema'}))
        bad = self.declaration('a', self.a, '../escape')
        with self.assertRaises(ValueError):
            declare(bad)
        bad = self.declaration('a', self.a, 'src/')
        bad['change_summary'] = ''
        with self.assertRaises(ValueError):
            declare(bad)
        with self.assertRaises(ValueError):
            request({'action': 'schema', 'unknown': True})

    def test_intent_review_isolates_hunks_and_does_not_replace_main(self):
        declare(self.declaration('a', self.a, 'src/a.txt'))
        self.commit(self.a, 'src/a.txt', 'intentional feature\n')
        self.commit(self.a, 'src/b.txt', 'unrelated change\n')
        params = {'worktree': str(self.main), 'main_ref': 'refs/heads/main', 'worktrees': [str(self.a)]}
        snapshot = inspect(params)
        tree = snapshot['worktrees'][0]
        reviews = []
        for change in tree['changes']:
            for hunk in change['hunks']:
                reviews.append({'worktree': str(self.a), 'head': tree['head'], 'owner_actor_id': tree['owner']['id'], 'owner_generation': tree['owner']['generation'],
                                'hunk_id': hunk['id'], 'decision': 'include' if change['path'] == 'src/a.txt' else 'exclude',
                                'reason': 'Assigned feature' if change['path'] == 'src/a.txt' else 'Unrelated to task'})
        plan = evaluate({'snapshot_request': params, 'main_revision': snapshot['main_revision'], 'reviews': reviews})
        self.assertEqual(plan['status'], 'review_plan')
        self.assertEqual(len(plan['isolated_hunks']), 1)
        integration = self.root / 'integration'
        git(self.main, 'worktree', 'add', '-q', '--detach', str(integration), 'main')
        patch_file = self.root / 'selected.patch'
        patch_file.write_text(plan['isolated_hunks'][0]['patch'], encoding='utf8')
        git(integration, 'apply', '--check', str(patch_file))
        git(integration, 'apply', str(patch_file))
        self.assertEqual((integration / 'src/a.txt').read_text(), 'intentional feature\n')
        self.assertEqual((integration / 'src/b.txt').read_text(), 'baseline\n')
        self.assertEqual((self.main / 'src/a.txt').read_text(), 'baseline\n')
        self.assertEqual((self.a / 'src/b.txt').read_text(), 'unrelated change\n')

    def test_main_overlap_and_dirty_work_are_never_ready_for_inclusion(self):
        declare(self.declaration('a', self.a, 'src/a.txt'))
        self.commit(self.a, 'src/a.txt', 'feature\n')
        self.commit(self.main, 'src/a.txt', 'main work\n')
        params = {'worktree': str(self.main), 'main_ref': 'refs/heads/main', 'worktrees': [str(self.a)]}
        snapshot = inspect(params)
        tree = snapshot['worktrees'][0]
        self.assertEqual(tree['changes'][0]['category'], 'main_also_changed')
        self.assertEqual(evaluate({'snapshot_request': params, 'main_revision': snapshot['main_revision'], 'reviews': []})['status'], 'needs_review')
        (self.a / 'dirty.txt').write_text('ongoing work')
        self.assertTrue(inspect(params)['worktrees'][0]['dirty'])
        with self.assertRaisesRegex(ValueError, 'Main changed'):
            evaluate({'snapshot_request': params, 'main_revision': '0' * 40, 'reviews': []})

    def test_mode_only_change_is_preserved_for_review(self):
        declare(self.declaration('a', self.a, 'src/a.txt'))
        git(self.a, 'update-index', '--chmod=+x', '--', 'src/a.txt')
        git(self.a, 'commit', '-qm', 'mode change')
        snapshot = inspect({'worktree': str(self.main), 'main_ref': 'refs/heads/main', 'worktrees': [str(self.a)]})
        change = snapshot['worktrees'][0]['changes'][0]
        self.assertEqual(change['category'], 'candidate_for_review')
        self.assertNotEqual(change['main_mode'], change['proposed_mode'])
        self.assertTrue(change['isolation_supported'])
        self.assertEqual(change['hunks'][0]['unit_kind'], 'file_change')

    def test_binary_and_empty_file_operations_preserve_main_and_exact_bytes(self):
        declare(self.declaration('a', self.a, 'src/'))
        binary = bytes(range(256)) * 4
        (self.a / 'src/asset.bin').write_bytes(binary)
        (self.a / 'src/empty.txt').write_bytes(b'')
        git(self.a, 'add', '--', 'src/asset.bin', 'src/empty.txt')
        git(self.a, 'commit', '-qm', 'binary and empty artifacts')
        params = {'worktree': str(self.main), 'main_ref': 'refs/heads/main', 'worktrees': [str(self.a)]}
        snapshot = inspect(params)
        result = evaluate({'snapshot_request': params, 'main_revision': snapshot['main_revision'], 'reviews': self.include_reviews(snapshot)})
        self.assertEqual(result['status'], 'review_plan')
        self.assertEqual(len(result['isolated_files']), 2)
        integration = self.root / 'binary integration'
        git(self.main, 'worktree', 'add', '-q', '--detach', str(integration), 'main')
        for operation in result['isolated_files']:
            git(integration, *operation['integration_only_git_args'])
        self.assertEqual((integration / 'src/asset.bin').read_bytes(), binary)
        self.assertEqual((integration / 'src/empty.txt').read_bytes(), b'')
        self.assertFalse((self.main / 'src/asset.bin').exists())
        self.assertEqual((self.a / 'src/asset.bin').read_bytes(), binary)

    def test_overlapping_selected_worktrees_require_combined_review(self):
        for owner, tree, content in [('a', self.a, 'first feature\n'), ('b', self.b, 'second feature\n')]:
            declare(self.declaration(owner, tree, 'src/a.txt'))
            self.commit(tree, 'src/a.txt', content)
            release({'host': 'codex', 'session_id': owner, 'reason': 'Implementation finished; preserve worktree for integration'})
        params = {'worktree': str(self.main), 'main_ref': 'refs/heads/main', 'worktrees': [str(self.a), str(self.b)]}
        snapshot = inspect(params)
        result = evaluate({'snapshot_request': params, 'main_revision': snapshot['main_revision'], 'reviews': self.include_reviews(snapshot)})
        self.assertEqual(result['status'], 'needs_review')
        self.assertFalse(result['isolated_hunks'])
        self.assertTrue(any('multiple worktrees' in gap['reason'] for gap in result['unresolved']))
        self.assertEqual((self.a / 'src/a.txt').read_text(), 'first feature\n')
        self.assertEqual((self.b / 'src/a.txt').read_text(), 'second feature\n')

    def test_scope_redeclaration_retains_intent_history(self):
        declare(self.declaration('a', self.a, 'src/a.txt'))
        value = self.declaration('a', self.a, 'src/b.txt')
        value['change_summary'] = 'A separately assigned follow-up for file b.'
        declare(value)
        saved = status({'worktree': str(self.a), 'include_history': True})
        self.assertEqual(len(saved['intent_history']), 2)
        self.assertTrue(any(event['payload']['scopes'][0]['path'] == 'src/a.txt' for event in saved['intent_history']))

    def test_deployment_preserves_other_hooks_and_is_idempotent(self):
        from deploy import install
        codex, claude = self.root / 'codex', self.root / 'claude'
        codex.mkdir()
        claude.mkdir()
        original = {'hooks': {'PreToolUse': [{'hooks': [{'type': 'command', 'command': 'existing-hook'}]}]}, 'unrelated': {'keep': True}}
        (codex / 'hooks.json').write_text(json.dumps(original))
        (claude / 'settings.json').write_text(json.dumps(original))
        runtime = self.root / 'runtime'
        install(runtime, codex, claude)
        first = (codex / 'hooks.json').read_bytes()
        install(runtime, codex, claude)
        self.assertEqual(first, (codex / 'hooks.json').read_bytes())
        result = json.loads(first)
        self.assertEqual(result['hooks']['PreToolUse'][0], original['hooks']['PreToolUse'][0])
        self.assertEqual(result['unrelated'], original['unrelated'])


if __name__ == '__main__':
    unittest.main()

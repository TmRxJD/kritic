"""Unavailable Claude owner → actual successor, with preserved Git and dirty state."""
import concurrent.futures
import copy
import sys
import unittest
from pathlib import Path

sys.path.insert(0,str(Path(__file__).resolve().parent))
import test_coordination as fixtures
from coordinator import request as dispatch
from hooks import hook
from paths import git
from recovery import recover
from registry import actor_id, declare, status, transaction
from transfers import snapshot
import test_transfers as transfer_fixtures


class RecoveryTests(unittest.TestCase):
    setUp = fixtures.CoordinationTests.setUp
    declaration = fixtures.CoordinationTests.declaration
    event = fixtures.CoordinationTests.event

    def inspect(self, successor='fresh', host='codex'):
        return recover({'phase':'inspect','host':host,'session_id':successor,
            'worktree':str(self.a),'source_actor_id':self.source['actor_id'],
            'expected_generation':self.source['generation'],
            'authorization':{'kind':'user_request','user_request':'Claude hit its token limit; let Codex take over this tree.',
                             'reason':'Source cannot release or prepare a handoff.'}})

    def prepare(self):
        self.source = declare(self.declaration('source',self.a,'src/a.txt',host='claude'))
        (self.a/'src/a.txt').write_text('staged progress\n')
        git(self.a,'add','--','src/a.txt')
        (self.a/'src/a.txt').write_text('valuable unstaged continuation\n')
        (self.a/'unfinished.dat').write_bytes(b'valuable untracked partial result')
        hook('claude','end',self.event('source',self.a))
        return self.inspect()

    def accept(self, inspected, successor='fresh', host='codex'):
        return recover({'phase':'accept','host':host,'session_id':successor,
            'plan':inspected['plan'],'plan_sha256':inspected['plan_sha256'],
            'acknowledge_context_gaps':True})

    def test_recovery_preserves_all_work_and_fences_unavailable_source(self):
        inspected = self.prepare()
        before = snapshot(self.a)
        self.assertEqual(inspected['status'],'review_required')
        self.assertEqual(status({'worktree':str(self.a)})['actors'][0]['claim_state'],'active')
        result = self.accept(inspected)
        self.assertTrue(result['writer_transferred'])
        self.assertFalse(result['authorization_authenticated'])
        self.assertEqual(snapshot(self.a),before)
        self.assertEqual(result['scopes'],self.source['declaration']['scopes'])
        self.assertEqual(result['task'],self.source['declaration']['task'])
        self.assertTrue(result['context_gaps'])
        self.assertIn('deny',str(hook('claude','pretool',self.event('source',self.a))))
        hook('claude','end',self.event('source',self.a))
        hook('claude','session',self.event('source',self.a))
        with self.assertRaisesRegex(ValueError,'transferred'):
            declare(self.declaration('source',self.a,'src/a.txt',host='claude'))
        self.assertNotIn('deny',str(hook('codex','pretool',self.event('fresh',self.a))))
        hook('codex','end',self.event('fresh',self.a))
        hook('codex','session',self.event('fresh',self.a))
        self.assertNotIn('deny',str(hook('codex','pretool',self.event('fresh',self.a))))
        with self.assertRaises(ValueError):
            self.accept(inspected)

    def test_wrong_successor_and_unacknowledged_gaps_do_not_change_claims(self):
        inspected = self.prepare()
        with self.assertRaisesRegex(ValueError,'successor'):
            self.accept(inspected,'wrong')
        with self.assertRaisesRegex(ValueError,'acknowledgement'):
            recover({'phase':'accept','host':'codex','session_id':'fresh','plan':inspected['plan'],
                'plan_sha256':inspected['plan_sha256'],'acknowledge_context_gaps':False})
        self.assertEqual(status({'worktree':str(self.a)})['actors'][0]['claim_state'],'active')

    def test_missing_or_implicit_authorization_is_rejected(self):
        self.prepare()
        packet={'phase':'inspect','host':'codex','session_id':'fresh','worktree':str(self.a),
            'source_actor_id':self.source['actor_id'],'expected_generation':self.source['generation'],
            'authorization':{'kind':'heartbeat_expired','user_request':'','reason':'inactive'}}
        with self.assertRaisesRegex(ValueError,'explicit user'):
            recover(packet)
        packet['authorization']={'kind':'user_request','user_request':'','reason':'token limit'}
        with self.assertRaises(ValueError):
            recover(packet)

    def test_task_or_scope_changes_even_with_recomputed_hash_are_rejected(self):
        from recovery import digest
        inspected = self.prepare()
        for field,value in [('task','invented task'),('scopes',[{'path':'src/','purpose':'expanded'}])]:
            changed=copy.deepcopy(inspected)
            changed['plan']['source_declaration'][field]=value
            changed['plan_sha256']=digest(changed['plan'])
            with self.assertRaisesRegex(ValueError,'evidence changed'):
                self.accept(changed)

    def test_dirty_and_index_drift_are_rejected(self):
        inspected = self.prepare()
        (self.a/'src/a.txt').write_text('new source work\n')
        with self.assertRaisesRegex(ValueError,'evidence changed'):
            self.accept(inspected)
        inspected = self.inspect()
        git(self.a,'add','--','src/a.txt')
        with self.assertRaisesRegex(ValueError,'evidence changed'):
            self.accept(inspected)
        self.assertEqual((self.a/'src/a.txt').read_text(),'new source work\n')

    def test_new_source_event_invalidates_plan_without_needing_a_handoff(self):
        inspected = self.prepare()
        hook('claude','prompt',self.event('source',self.a))
        with self.assertRaisesRegex(ValueError,'evidence changed'):
            self.accept(inspected)
        self.assertTrue(self.accept(self.inspect())['writer_transferred'])

    def test_generation_change_and_other_destination_assignment_are_rejected(self):
        inspected = self.prepare()
        self.source = declare(self.declaration('source',self.a,'src/a.txt',host='claude'))
        with self.assertRaisesRegex(ValueError,'generation changed'):
            self.accept(inspected)
        declare(self.declaration('fresh',self.b,'src/b.txt'))
        with self.assertRaisesRegex(ValueError,'another worktree'):
            self.inspect()

    def test_another_agent_in_the_source_session_is_not_a_fresh_successor(self):
        self.prepare()
        with self.assertRaisesRegex(ValueError,'different actual session'):
            recover({'phase':'inspect','host':'claude','session_id':'source','agent_id':'child',
                'worktree':str(self.a),'source_actor_id':self.source['actor_id'],
                'expected_generation':self.source['generation'],
                'authorization':{'kind':'user_request','user_request':'Continue work.','reason':'token limit'}})

    def test_two_successors_have_exactly_one_winner(self):
        first = self.prepare()
        second = self.inspect('other')
        def accept(pair):
            try:
                return self.accept(pair[0],pair[1])['writer_transferred']
            except ValueError:
                return False
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results=list(pool.map(accept,[(first,'fresh'),(second,'other')]))
        self.assertEqual(sorted(results),[False,True])

    def test_pending_source_transfer_is_invalidated_and_cannot_later_accept(self):
        fixture = transfer_fixtures.TransferTests('test_accept_keeps_exact_worktree_scopes_and_dirty_bytes')
        # Reuse the current isolated registry/tree; create an ordinary pending transfer.
        fixture.a=self.a
        fixture.root=self.root
        fixture.setup_transfer()
        with transaction() as db:
            source_id=actor_id('codex','source')
            generation=db.execute('SELECT generation FROM actors WHERE id=?',(source_id,)).fetchone()[0]
        self.source={'actor_id':source_id,'generation':generation}
        inspected=self.inspect()
        self.assertTrue(self.accept(inspected)['writer_transferred'])
        with transaction() as db:
            self.assertEqual(db.execute('SELECT status FROM transfers WHERE id=?',(fixture.prepared['transfer_id'],)).fetchone()[0],'cancelled')
        from transfers import transfer
        with self.assertRaises(ValueError):
            transfer(fixture.accept_request)

    def test_discovery_and_hook_bootstrap_support_undeclared_actual_successor(self):
        inspected=self.prepare()
        self.assertIn('recover',dispatch({'action':'schema'}))
        event=self.event('fresh',self.a,tool='mcp__kev_tools__worktree_evaluate')
        event['tool_input']={'action':'recover','request':{'phase':'inspect','host':'codex','session_id':'fresh'}}
        self.assertNotIn('deny',str(hook('codex','pretool',event)))
        event['tool_input']['request']['session_id']='source'
        self.assertIn('deny',str(hook('codex','pretool',event)))
        import json,shlex
        coordinator=Path(__file__).resolve().parent/'coordinator.py'
        packet={'action':'recover','request':{'phase':'accept','host':'codex','session_id':'fresh',
            'plan':inspected['plan'],'plan_sha256':inspected['plan_sha256'],'acknowledge_context_gaps':True}}
        event=self.event('fresh',self.a,tool='exec_command')
        event['tool_input']={'cmd':f'python -I "{coordinator.as_posix()}" --request-json '+shlex.quote(json.dumps(packet))}
        self.assertNotIn('deny',str(hook('codex','pretool',event)))


if __name__ == '__main__':
    unittest.main()

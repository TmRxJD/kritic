"""Actual transactional succession of one existing worktree, including dirty work."""
import concurrent.futures
import hashlib
import json
import sqlite3
from pathlib import Path
import sys
import unittest

sys.path.insert(0,str(Path(__file__).resolve().parent))
import test_coordination as fixtures
from hooks import hook
from registry import actor_id, declare, release, status, transaction
from transfers import transfer


class TransferTests(unittest.TestCase):
    setUp = fixtures.CoordinationTests.setUp
    declaration = fixtures.CoordinationTests.declaration
    event = fixtures.CoordinationTests.event
    def pin(self,path,value):
        path.write_bytes(json.dumps(value).encode())
        return {'path':str(path.resolve()),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}

    def setup_transfer(self, bind=True):
        source = declare(self.declaration('source',self.a,'src/a.txt'))
        self.dirty = self.a/'src/a.txt'
        self.dirty.write_text('valuable uncommitted progress\n',encoding='utf8')
        handoff = self.pin(self.root/'handoff.json',{'source_session_id':'source','protected':['u1'],'facts':['f1']})
        self.report = self.pin(self.root/'handoff-review.json',{'status':'covered','ready_for_controller_review':True,'source_session_id':'source',
            'coverage_gaps':[],'findings':[],'worktree':str(self.a),'scope':['src/a.txt'],
            'handoff_path':handoff['path'],'handoff_sha256':handoff['sha256'],'resume_pins':[handoff],
            'workspace_revision':source['declaration']['declared_head'],'required_user_event_ids':['u1'],'required_fact_ids':['f1'],'required_fixture_ids':[]})
        hook('codex','boundary',self.event('source',self.a))
        self.prepared = transfer({'phase':'prepare','host':'codex','session_id':'source','expected_generation':source['generation'],'handoff_report':self.report})
        if not bind:
            return
        self.bound = transfer({'phase':'bind','host':'codex','session_id':'source','transfer_id':self.prepared['transfer_id'],
            'expected_transfer_generation':1,'destination':{'host':'codex','session_id':'fresh'}})
        self.accept_request = {'phase':'accept','host':'codex','session_id':'fresh','transfer_id':self.prepared['transfer_id'],
            'expected_transfer_generation':2,'handoff_sha256':self.prepared['handoff_sha256'],
            'accepted_user_event_ids':['u1'],'accepted_fact_ids':['f1'],'accepted_fixture_ids':[]}

    def test_accept_keeps_exact_worktree_scopes_and_dirty_bytes(self):
        self.setup_transfer()
        before = self.dirty.read_bytes()
        result = transfer(self.accept_request)
        self.assertEqual(result['status'],'accepted')
        self.assertEqual(Path(result['worktree']).resolve(),self.a.resolve())
        self.assertEqual(result['scopes'],[{'path':'src/a.txt','purpose':'Implement the assigned feature in this location'}])
        self.assertEqual(self.dirty.read_bytes(),before)
        self.assertEqual(hook('codex','pretool',self.event('fresh',self.a)),{})
        self.assertEqual(hook('codex','pretool',self.event('source',self.a))['hookSpecificOutput']['permissionDecision'],'deny')

    def test_source_close_and_reopen_cannot_reclaim_or_deadlock_successor(self):
        self.setup_transfer()
        hook('codex','end',self.event('source',self.a))
        transfer(self.accept_request)
        hook('codex','session',self.event('source',self.a))
        self.assertEqual(hook('codex','pretool',self.event('fresh',self.a)),{})
        with self.assertRaisesRegex(ValueError,'transferred'):
            declare(self.declaration('source',self.a,'src/a.txt'))

    def test_pending_reserves_tree_and_blocks_source_mutation(self):
        self.setup_transfer()
        with self.assertRaisesRegex(ValueError,'pending'):
            declare(self.declaration('unrelated',self.a,'src/b.txt'))
        self.assertEqual(hook('codex','pretool',self.event('source',self.a))['hookSpecificOutput']['permissionDecision'],'deny')

    def test_changed_workspace_and_new_prompt_invalidate_acceptance(self):
        self.setup_transfer()
        self.dirty.write_text('newer progress must survive\n',encoding='utf8')
        with self.assertRaisesRegex(ValueError,'changed'):
            transfer(self.accept_request)
        self.assertEqual(self.dirty.read_text(),'newer progress must survive\n')
        self.dirty.write_text('valuable uncommitted progress\n',encoding='utf8')
        hook('codex','prompt',self.event('source',self.a))
        with self.assertRaisesRegex(ValueError,'boundary|event'):
            transfer(self.accept_request)

    def test_wrong_ack_missing_context_and_unbound_actor_never_take_over(self):
        self.setup_transfer()
        for replacement in [{'handoff_sha256':'0'*64},{'accepted_user_event_ids':[]},{'session_id':'other-fresh'}]:
            with self.assertRaises(ValueError):
                transfer(dict(self.accept_request,**replacement))
        rows = status({'worktree':str(self.a)})['actors']
        source = next(row for row in rows if row['session']=='source')
        self.assertEqual(source['declaration'].get('access','edit'),'edit')

    def test_only_one_accept_wins_and_cancel_preserves_source(self):
        self.setup_transfer()
        def accept():
            try:
                return transfer(self.accept_request)['status']
            except ValueError:
                return 'rejected'
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(pool.map(lambda _:accept(),range(2)))
        self.assertEqual(sorted(outcomes),['accepted','rejected'])

    def test_cancel_before_ack_leaves_writer_and_files_untouched(self):
        self.setup_transfer()
        before = self.dirty.read_bytes()
        result = transfer({'phase':'cancel','host':'codex','session_id':'source','transfer_id':self.prepared['transfer_id'],
            'expected_transfer_generation':2,'reason':'Successor startup failed; retain source ownership'})
        self.assertFalse(result['writer_transferred'])
        self.assertEqual(self.dirty.read_bytes(),before)
        self.assertEqual(hook('codex','pretool',self.event('source',self.a)),{})
        with self.assertRaises(ValueError):
            transfer(self.accept_request)

    def test_undeclared_crashed_peer_does_not_freeze_current_owner(self):
        self.setup_transfer()
        hook('codex','session',self.event('unstarted-peer',self.b))
        transfer(self.accept_request)
        self.assertEqual(hook('codex','pretool',self.event('fresh',self.a)),{})
        self.assertEqual(hook('codex','pretool',self.event('unstarted-peer',self.b))['hookSpecificOutput']['permissionDecision'],'deny')

    def test_successor_end_and_resume_preserve_durable_claim_and_generation(self):
        self.setup_transfer()
        accepted = transfer(self.accept_request)
        hook('codex','end',self.event('fresh',self.a))
        owner = next(row for row in status({'worktree':str(self.a)})['actors'] if row['session']=='fresh')
        self.assertEqual(owner['generation'],accepted['owner_generation'])
        self.assertEqual(owner['claim_state'],'active')
        self.assertIn('durable claim retained',owner['presence'])
        with self.assertRaisesRegex(ValueError,'reserved'):
            declare(self.declaration('unrelated',self.a,'src/a.txt'))
        hook('codex','session',self.event('fresh',self.a))
        owner = next(row for row in status({'worktree':str(self.a)})['actors'] if row['session']=='fresh')
        self.assertEqual(owner['generation'],accepted['owner_generation'])
        self.assertEqual(hook('codex','pretool',self.event('fresh',self.a)),{})

    def test_multiple_successions_fence_every_previous_writer(self):
        self.setup_transfer()
        first = transfer(self.accept_request)
        hook('codex','boundary',self.event('fresh',self.a))
        report = json.loads(Path(self.report['path']).read_bytes())
        report['source_session_id'] = 'fresh'
        second_report = self.pin(self.root/'second-review.json',report)
        second = transfer({'phase':'prepare','host':'codex','session_id':'fresh',
            'expected_generation':first['owner_generation'],'handoff_report':second_report})
        transfer({'phase':'bind','host':'codex','session_id':'fresh','transfer_id':second['transfer_id'],
            'expected_transfer_generation':1,'destination':{'host':'codex','session_id':'third'}})
        transfer(dict(self.accept_request,session_id='third',transfer_id=second['transfer_id']))
        for previous in ('source','fresh'):
            hook('codex','end',self.event(previous,self.a))
            hook('codex','session',self.event(previous,self.a))
            self.assertEqual(hook('codex','pretool',self.event(previous,self.a))['hookSpecificOutput']['permissionDecision'],'deny')
            with self.assertRaisesRegex(ValueError,'transferred'):
                declare(self.declaration(previous,self.a,'src/a.txt'))
        self.assertEqual(hook('codex','pretool',self.event('third',self.a)),{})

    def test_only_successor_can_release_accepted_assignment(self):
        self.setup_transfer()
        accepted = transfer(self.accept_request)
        self.assertEqual(release({'host':'codex','session_id':'source','reason':'Source runtime finished'})['status'],'transferred')
        release({'host':'codex','session_id':'fresh','expected_generation':accepted['owner_generation'],'reason':'Explicitly completed assigned work'})
        declare(self.declaration('next-task',self.a,'src/a.txt'))
        self.assertEqual(self.dirty.read_text(),'valuable uncommitted progress\n')

    def test_bind_rejects_same_session_with_different_agent(self):
        self.setup_transfer(bind=False)
        with self.assertRaisesRegex(ValueError,'fresh successor'):
            transfer({'phase':'bind','host':'codex','session_id':'source','transfer_id':self.prepared['transfer_id'],
                'expected_transfer_generation':1,'destination':{'host':'codex','session_id':'source','agent_id':'different-agent'}})

    def test_upgrade_recovers_ended_accepted_owner_from_intent_history(self):
        self.setup_transfer()
        accepted = transfer(self.accept_request)
        hook('codex','end',self.event('fresh',self.a))
        with transaction() as db:
            db.execute('DELETE FROM claim_states')
        owner = next(row for row in status({'worktree':str(self.a)})['actors'] if row['session']=='fresh')
        self.assertEqual(owner['claim_state'],'active')
        self.assertEqual(owner['generation'],accepted['owner_generation'])
        source = next(row for row in status({'worktree':str(self.a)})['actors'] if row['session']=='source')
        self.assertEqual(source['claim_state'],'transferred')
        opaque = self.event('source',self.a,tool='exec_command')
        opaque['tool_input'] = {'cmd':'git status','workdir':str(self.a)}
        self.assertEqual(hook('codex','pretool',opaque)['hookSpecificOutput']['permissionDecision'],'deny')
        hook('codex','session',self.event('fresh',self.a))
        self.assertEqual(hook('codex','pretool',self.event('fresh',self.a)),{})

    def test_old_database_writes_cannot_steal_ended_or_transferred_claims(self):
        self.setup_transfer()
        transfer(self.accept_request)
        hook('codex','end',self.event('fresh',self.a))
        source_key = actor_id('codex','source')
        with self.assertRaisesRegex(sqlite3.IntegrityError,'Transferred source|Durable writer reservation'):
            with transaction() as db:
                value = dict(self.declaration('source',self.a,'src/a.txt'),access='edit')
                db.execute('UPDATE actors SET declaration=? WHERE id=?',(json.dumps(value),source_key))
        # Model the old client inserting an actor then declaring while it ignores
        # the ended owner. Database enforcement must reject the second writer.
        with self.assertRaisesRegex(sqlite3.IntegrityError,'Durable writer reservation'):
            with transaction() as db:
                row = dict(db.execute('SELECT * FROM actors WHERE id=?',(source_key,)).fetchone())
                row.update(id=actor_id('codex','legacy'),session='legacy',declaration=json.dumps(self.declaration('legacy',self.a,'src/a.txt')))
                db.execute('INSERT INTO actors VALUES (?,?,?,?,?,?,?,?,?,?)',tuple(row.values()))
        self.assertEqual(self.dirty.read_text(),'valuable uncommitted progress\n')

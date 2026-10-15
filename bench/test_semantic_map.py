import json
import copy
from pathlib import Path
import re
import tempfile
import unittest

import semantic_map as sm


class SemanticMapTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root/'one.rs').write_text('fn authorize_token() { reject_invalid_token(); }\n', encoding='utf-8')
        (self.root/'two.md').write_text('Rendering color palettes and texture assets.\n', encoding='utf-8')
        self.corpus = sm.prepare(self.root, {'files': ['one.rs', 'two.md'], 'source_head': 'abc'})
        self.qrels = {'queries': [{'id': 'auth', 'query': 'authorize token', 'split': 'test',
                                  'critical': True, 'relevant': [{'path': 'one.rs', 'contains': 'reject_invalid_token'}]}]}

    def test_bm25_selects_relevant_document(self):
        ranking = sm.bm25(self.corpus['chunks'], 'authorize token')
        self.assertEqual(self.corpus['chunks'][ranking[0]]['path'], 'one.rs')
        result = sm.evaluate(self.corpus, self.qrels)
        self.assertEqual(result['summaries'][0]['mrr_at_10'], 1)
        self.assertTrue(result['summaries'][0]['passes_quality'])

    def test_qrels_missing_literal_fails_closed(self):
        self.qrels['queries'][0]['relevant'][0]['contains'] = 'missing_symbol'
        with self.assertRaisesRegex(ValueError, 'Missing gold target'):
            sm.gold_targets(self.corpus, self.qrels)

    def test_escaped_and_excluded_paths(self):
        for path in ('../one.rs', '/one.rs', 'C:/one.rs', 'nested/../one.rs', 'one\\x.rs', '.git/config.json', 'node_modules/a.rs', 'one.py'):
            with self.subTest(path=path), self.assertRaises(ValueError):
                sm.safe_source(self.root, path)

    def test_symlink_rejected(self):
        try:
            (self.root/'link.rs').symlink_to(self.root/'one.rs')
        except OSError:
            self.skipTest('Symlink creation unavailable on this host')
        with self.assertRaisesRegex(ValueError, 'Symlink'):
            sm.safe_source(self.root, 'link.rs')

    def test_vectors_reject_nonfinite_and_malformed(self):
        for value in ([float('nan')], [float('inf')], [1e308, 1e308], [True], ['1'], [], [0, 0], None):
            with self.subTest(value=value), self.assertRaises(ValueError):
                sm.vector(value)
        with self.assertRaisesRegex(ValueError, 'dimension'):
            sm.vector([1, 2], 3)
        self.assertEqual(sm.vector([3, 4]), [.6, .8])

    def test_fusion_rewards_shared_high_rank(self):
        chunks = [{'id': str(i)} for i in range(3)]
        self.assertEqual(sm.fusion(chunks, [[0, 1, 2], [1, 2, 0]])[0], 1)

    def test_reproducible_chunks_and_source_changes(self):
        second = sm.prepare(self.root, {'files': ['two.md', 'one.rs'], 'source_head': 'abc'})
        self.assertEqual(self.corpus['corpus_hash'], second['corpus_hash'])
        self.assertEqual(self.corpus['chunks'], second['chunks'])
        (self.root/'one.rs').write_text('changed\n', encoding='utf-8')
        changed = sm.prepare(self.root, {'files': ['one.rs', 'two.md'], 'source_head': 'abc'})
        self.assertNotEqual(self.corpus['corpus_hash'], changed['corpus_hash'])

    def test_index_model_and_freshness_reject(self):
        config = {'id': 'model', 'revision': 'a'*40}
        metadata = {'type': 'metadata', 'schema': sm.SCHEMA, 'corpus_hash': self.corpus['corpus_hash'],
                    'source_head': 'abc', 'model': sm.config_identity(config), 'dimension': 2}
        rows = [metadata]+[{'type': 'chunk', 'chunk': c, 'vector': [1, 0]} for c in self.corpus['chunks']]
        path = self.root/'index.jsonl'
        path.write_text('\n'.join(json.dumps(r) for r in rows), encoding='utf-8')
        self.assertEqual(len(sm.load_index(path, self.corpus, config)['vectors']), 2)
        with self.assertRaisesRegex(ValueError, 'model mismatch'):
            sm.load_index(path, self.corpus, {'id': 'other', 'revision': 'a'*40})
        with self.assertRaisesRegex(ValueError, 'model mismatch'):
            sm.load_index(path, self.corpus, dict(config, quantization='dynamic-int8'))
        stale = dict(self.corpus, source_head='changed')
        with self.assertRaisesRegex(ValueError, 'freshness'):
            sm.load_index(path, stale, config)
        rows[1]['vector'] = [float('nan'), 1]
        path.write_text('\n'.join(json.dumps(r) for r in rows), encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'nonfinite'):
            sm.load_index(path, self.corpus, config)

    def test_live_source_freshness_reject(self):
        sm.validate_corpus(self.corpus, self.root)
        (self.root/'one.rs').write_text('changed\n', encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'freshness'):
            sm.validate_corpus(self.corpus, self.root)

    def test_reduced_dimension_requires_declared_support(self):
        with self.assertRaisesRegex(ValueError, 'Matryoshka'):
            sm.config_identity({'id': 'MiniLM', 'revision': 'a'*40, 'dimensions': 8})

    def test_revision_and_quantization_fail_closed(self):
        with self.assertRaisesRegex(ValueError, 'immutable 40-hex'):
            sm.config_identity({'id': 'model', 'revision': 'main'})
        with self.assertRaisesRegex(ValueError, 'Unsupported quantization'):
            sm.config_identity({'id': 'model', 'revision': 'a'*40, 'quantization': 'arbitrary'})

    def test_test_split_is_not_eligible_for_selection(self):
        result = sm.evaluate(self.corpus, self.qrels)
        self.assertEqual(result['selection_split'], 'dev')
        self.assertFalse(result['summaries'][0]['eligible_for_selection'])

    def test_attention_mask_mean_and_cls_pooling(self):
        hidden = [[[3., 0.], [0., 4.], [100., 100.]]]
        mask = [[1, 1, 0]]
        self.assertEqual(sm.pool_hidden(hidden, mask, 'mean'), [[.6, .8]])
        self.assertEqual(sm.pool_hidden(hidden, mask, 'cls'), [[1., 0.]])
        self.assertEqual(sm.pool_hidden([[3., 4.]], mask, 'mean'), [[.6, .8]])
        with self.assertRaisesRegex(ValueError, 'mask'):
            sm.pool_hidden(hidden, [[0, 0, 0]], 'mean')

    def test_onnx_identity_and_explicit_filename(self):
        base = {'id': 'model', 'revision': 'a'*40}
        onnx = dict(base, backend='onnx', onnx_file='onnx/model.onnx', pooling='mean')
        self.assertNotEqual(sm.config_identity(base), sm.config_identity(onnx))
        self.assertNotEqual(sm.config_identity(onnx), sm.config_identity(dict(onnx, parameter_quantization='int8')))
        for filename in ('../model.onnx', '/model.onnx', 'C:/model.onnx', 'onnx\\model.onnx'):
            with self.subTest(filename=filename), self.assertRaises(ValueError):
                sm.config_identity(dict(onnx, onnx_file=filename))

    def test_token_offset_windows_preserve_exact_source(self):
        text = 'alpha beta\ngamma delta\nepsilon zeta\n'
        offsets = [m.span() for m in re.finditer(r'\S+', text)]
        chunks = sm.chunk_token_offsets('one.rs', text, offsets, 3, 1)
        self.assertEqual([c['text'] for c in chunks],
                         ['alpha beta\ngamma', 'gamma delta\nepsilon', 'epsilon zeta'])
        self.assertEqual([(c['start'], c['end']) for c in chunks], [(1, 2), (2, 3), (3, 3)])
        for chunk in chunks:
            self.assertEqual(text[chunk['char_start']:chunk['char_end']], chunk['text'])
        with self.assertRaisesRegex(ValueError, 'Token limit'):
            sm.chunk_token_offsets('one.rs', text, offsets, 3, 3)

    def test_token_corpus_rejects_tampered_range_and_live_changes(self):
        corpus = copy.deepcopy(self.corpus)
        corpus['tokenization'] = {'kind': 'tokens', 'tokenizer_sha256': 'a'*64,
                                  'max_tokens': 224, 'overlap_tokens': 32}
        for chunk in corpus['chunks']:
            text = (self.root/chunk['path']).read_bytes().decode('utf-8')
            chunk['char_start'], chunk['char_end'] = 0, len(text)
            chunk['id'] = sm.digest({k: v for k, v in chunk.items() if k != 'id'})
        corpus['corpus_hash'] = sm.digest(sm.corpus_identity(corpus))
        sm.validate_corpus(corpus, self.root)
        tampered = copy.deepcopy(corpus)
        tampered['chunks'][0]['char_end'] -= 1
        with self.assertRaisesRegex(ValueError, 'identity'):
            sm.validate_corpus(tampered, self.root)
        changed = tampered['chunks'][0]
        changed['id'] = sm.digest({k: v for k, v in changed.items() if k != 'id'})
        tampered['corpus_hash'] = sm.digest(sm.corpus_identity(tampered))
        with self.assertRaisesRegex(ValueError, 'range mismatch'):
            sm.validate_corpus(tampered, self.root)
        (self.root/'one.rs').write_text('changed\n', encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'freshness'):
            sm.validate_corpus(corpus, self.root)


if __name__ == '__main__':
    unittest.main()

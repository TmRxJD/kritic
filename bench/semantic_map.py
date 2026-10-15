"""Bounded, CPU-only retrieval research harness; never executes corpus content."""
from __future__ import annotations
import argparse
import collections
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import re
import statistics
import threading
import time

SCHEMA = 1
SUFFIXES = {'.rs', '.md', '.toml', '.json'}
EXCLUDED = {'.git', 'node_modules', 'dist', 'build', 'debug', 'target', '.turbo',
            'coverage', '__pycache__', 'graphify-out', '.venv', 'venv'}


def read_json(path):
    return json.loads(Path(path).read_text(encoding='utf-8'))


def write_json(path, value):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + '\n', encoding='utf-8')


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, ensure_ascii=False,
                                     separators=(',', ':')).encode()).hexdigest()


def safe_source(repo, relative):
    if not isinstance(relative, str) or '\\' in relative or ':' in relative:
        raise ValueError(f'Invalid relative source path: {relative!r}')
    parts = PurePosixPath(relative)
    if parts.is_absolute() or '..' in parts.parts or not parts.parts:
        raise ValueError(f'Escaped source path: {relative!r}')
    if str(parts) != relative or any(p.lower() in EXCLUDED for p in parts.parts):
        raise ValueError(f'Excluded/noncanonical source path: {relative!r}')
    if parts.suffix.lower() not in SUFFIXES:
        raise ValueError(f'Disallowed source suffix: {relative!r}')
    root = Path(repo).resolve(strict=True)
    current = root
    for part in parts.parts:
        current = current / part
        if current.is_symlink() or (hasattr(current, 'is_junction') and current.is_junction()):
            raise ValueError(f'Symlink/junction source rejected: {relative}')
    source = current.resolve(strict=True)
    if not source.is_relative_to(root) or not source.is_file():
        raise ValueError(f'Escaped/non-file source: {relative}')
    return source


def chunk_text(path, text, words=100, overlap=15):
    if words <= overlap or overlap < 0:
        raise ValueError('Chunk words must exceed nonnegative overlap')
    # Whole-line chunks preserve navigable source ranges. Long single lines remain intact.
    lines = text.splitlines(keepends=True)
    chunks, start = [], 0
    while start < len(lines):
        end, count = start, 0
        while end < len(lines) and (count < words or end == start):
            count += len(lines[end].split())
            end += 1
        content = ''.join(lines[start:end])
        if content.strip():
            row = {'path': path, 'start': start + 1, 'end': end, 'text': content,
                   'content_sha256': hashlib.sha256(content.encode()).hexdigest()}
            row['id'] = digest(row)
            chunks.append(row)
        if end == len(lines):
            break
        next_start, count = end, 0
        while next_start > start + 1 and count < overlap:
            next_start -= 1
            count += len(lines[next_start].split())
        start = next_start
    return chunks


def chunk_token_offsets(path, text, offsets, max_tokens=224, overlap_tokens=32):
    if max_tokens <= overlap_tokens or overlap_tokens < 0:
        raise ValueError('Token limit must exceed nonnegative overlap')
    previous = 0
    for start, end in offsets:
        if start < previous or end <= start or end > len(text):
            raise ValueError('Invalid tokenizer offsets')
        previous = start
    chunks = []
    for token_start in range(0, len(offsets), max_tokens-overlap_tokens):
        window = offsets[token_start:token_start+max_tokens]
        start, end = window[0][0], window[-1][1]
        content = text[start:end]
        row = {'path': path, 'start': text.count('\n', 0, start)+1,
               'end': text.count('\n', 0, end-1)+1, 'char_start': start, 'char_end': end,
               'text': content, 'content_sha256': hashlib.sha256(content.encode()).hexdigest()}
        row['id'] = digest(row)
        chunks.append(row)
        if token_start+max_tokens >= len(offsets):
            break
    return chunks


def corpus_identity(corpus):
    identity = {'sources': corpus['sources'], 'words': corpus['chunk_words'],
                'overlap': corpus['overlap_words'], 'schema': SCHEMA}
    if corpus.get('tokenization'):
        identity['tokenization'] = corpus['tokenization']
        identity['chunk_ids'] = [c['id'] for c in corpus['chunks']]
    return identity


def prepare(repo, manifest, words=100, overlap=15, tokenizer_path=None, max_tokens=224, overlap_tokens=32):
    files = manifest.get('files')
    if not isinstance(files, list) or not files or any(not isinstance(p, str) for p in files):
        raise ValueError('Manifest requires nonempty files: [relative-path strings]')
    if len(files) != len(set(files)):
        raise ValueError('Duplicate manifest files')
    chunks, sources = [], []
    tokenizer, tokenization = None, None
    if tokenizer_path:
        from tokenizers import Tokenizer
        tokenizer_bytes = Path(tokenizer_path).read_bytes()
        tokenizer = Tokenizer.from_str(tokenizer_bytes.decode('utf-8'))
        tokenizer.no_truncation()
        tokenizer.no_padding()
        tokenization = {'kind': 'tokens', 'tokenizer_sha256': hashlib.sha256(tokenizer_bytes).hexdigest(),
                        'max_tokens': max_tokens, 'overlap_tokens': overlap_tokens}
    for relative in sorted(files):
        raw = safe_source(repo, relative).read_bytes()
        sources.append({'path': relative, 'sha256': hashlib.sha256(raw).hexdigest()})
        text = raw.decode('utf-8')
        if tokenizer:
            offsets = tokenizer.encode(text, add_special_tokens=False).offsets
            chunks.extend(chunk_token_offsets(relative, text, offsets, max_tokens, overlap_tokens))
        else:
            chunks.extend(chunk_text(relative, text, words, overlap))
    if not chunks:
        raise ValueError('Empty corpus')
    corpus = {'schema': SCHEMA, 'sources': sources,
            'source_head': manifest.get('source_head'), 'chunk_words': words,
            'overlap_words': overlap, 'chunks': chunks,
            'metadata': manifest.get('metadata', {})}
    if tokenization:
        corpus['tokenization'] = tokenization
    corpus['corpus_hash'] = digest(corpus_identity(corpus))
    return corpus


def validate_corpus(corpus, repo=None):
    identity = corpus_identity(corpus)
    if corpus.get('schema') != SCHEMA or digest(identity) != corpus.get('corpus_hash'):
        raise ValueError('Corpus identity mismatch')
    if not corpus.get('chunks'):
        raise ValueError('Empty corpus')
    paths = {s['path'] for s in corpus['sources']}
    seen = set()
    for chunk in corpus['chunks']:
        row = {k: chunk[k] for k in ('path', 'start', 'end', 'text', 'content_sha256')}
        if corpus.get('tokenization'):
            if not all(isinstance(chunk.get(k), int) and not isinstance(chunk[k], bool) for k in ('char_start', 'char_end')):
                raise ValueError('Invalid corpus character range')
            row.update(char_start=chunk['char_start'], char_end=chunk['char_end'])
            if chunk['char_start'] < 0 or chunk['char_end'] <= chunk['char_start']:
                raise ValueError('Invalid corpus character range')
        if (chunk['path'] not in paths or chunk['id'] in seen or digest(row) != chunk['id']
                or hashlib.sha256(chunk['text'].encode()).hexdigest() != chunk['content_sha256']
                or chunk['start'] < 1 or chunk['end'] < chunk['start']):
            raise ValueError('Corpus chunk identity mismatch')
        seen.add(chunk['id'])
    if repo:
        if corpus.get('tokenization'):
            texts = {}
            for source in corpus['sources']:
                raw = safe_source(repo, source['path']).read_bytes()
                if hashlib.sha256(raw).hexdigest() != source['sha256']:
                    raise ValueError('Corpus source freshness mismatch')
                texts[source['path']] = raw.decode('utf-8')
            for chunk in corpus['chunks']:
                text = texts[chunk['path']]
                start, end = chunk['char_start'], chunk['char_end']
                if (end > len(text) or text[start:end] != chunk['text']
                        or text.count('\n', 0, start)+1 != chunk['start']
                        or text.count('\n', 0, end-1)+1 != chunk['end']):
                    raise ValueError('Corpus source range mismatch')
        else:
            fresh = prepare(repo, {'files': sorted(paths), 'source_head': corpus.get('source_head')},
                            corpus['chunk_words'], corpus['overlap_words'])
            if fresh['corpus_hash'] != corpus['corpus_hash'] or fresh['chunks'] != corpus['chunks']:
                raise ValueError('Corpus source freshness mismatch')
    return corpus


def tokens(text):
    return re.findall(r'[a-z0-9]+', text.lower())


def bm25(chunks, query):
    documents = [collections.Counter(tokens(c['text'])) for c in chunks]
    lengths = [sum(d.values()) for d in documents]
    average = sum(lengths) / len(lengths)
    scores = [0.0] * len(chunks)
    for term in set(tokens(query)):
        df = sum(term in d for d in documents)
        idf = math.log(1 + (len(chunks) - df + .5) / (df + .5))
        for i, document in enumerate(documents):
            frequency = document[term]
            if frequency:
                scores[i] += idf * frequency * 2.5 / (frequency + 1.5 * (.25 + .75 * lengths[i] / average))
    return sorted(range(len(chunks)), key=lambda i: (-scores[i], chunks[i]['id']))


def vector(value, dimension=None):
    if not isinstance(value, list) or not value or any(isinstance(x, bool) or not isinstance(x, (int, float)) or not math.isfinite(x) for x in value):
        raise ValueError('Malformed/nonfinite vector')
    if dimension is not None and len(value) != dimension:
        raise ValueError('Vector dimension mismatch')
    norm = math.sqrt(sum(x*x for x in value))
    if not math.isfinite(norm) or norm == 0:
        raise ValueError('Zero/nonfinite vector norm')
    return [x / norm for x in value]


def dense_rank(chunks, vectors, query_vector):
    query_vector = vector(query_vector, len(vectors[0]))
    return sorted(range(len(chunks)), key=lambda i: (-sum(a*b for a, b in zip(vectors[i], query_vector)), chunks[i]['id']))


def fusion(chunks, rankings, k=60):
    scores = collections.defaultdict(float)
    for ranking in rankings:
        for rank, index in enumerate(ranking, 1):
            scores[index] += 1 / (k + rank)
    return sorted(scores, key=lambda i: (-scores[i], chunks[i]['id']))


def gold_targets(corpus, qrels):
    queries = qrels.get('queries')
    if not isinstance(queries, list) or not queries:
        raise ValueError('Qrels requires nonempty queries')
    output, ids = {}, set()
    for query in queries:
        if query.get('id') in ids or not isinstance(query.get('id'), str) or not query['id']:
            raise ValueError('Invalid/duplicate query id')
        ids.add(query['id'])
        if query.get('split') not in ('dev', 'test') or not isinstance(query.get('query'), str) or not query['query'].strip():
            raise ValueError('Invalid query split/text')
        if not isinstance(query.get('critical', False), bool):
            raise ValueError('Query critical must be boolean')
        targets = query.get('relevant')
        if not isinstance(targets, list) or not targets:
            raise ValueError('Every query requires relevant targets')
        gold = set()
        for target in targets:
            literal = target.get('contains')
            if not isinstance(literal, str) or not literal:
                raise ValueError('Gold contains must be nonempty literal text')
            matched = {i for i, c in enumerate(corpus['chunks']) if c['path'] == target.get('path') and literal in c['text']}
            if not matched:
                raise ValueError(f"Missing gold target for {query['id']}: {target}")
            gold.update(matched)
        output[query['id']] = gold
    return output


def evaluate(corpus, qrels, index=None):
    gold = gold_targets(corpus, qrels)
    rows = []
    chunks = corpus['chunks']
    for query in qrels['queries']:
        lexical = bm25(chunks, query['query'])
        rankings = {'lexical': lexical}
        if index:
            if query['id'] not in index['queries'] or index['queries'][query['id']]['text'] != query['query']:
                raise ValueError('Index query set/text mismatch')
            dense = dense_rank(chunks, index['vectors'], index['queries'][query['id']]['vector'])
            rankings.update(dense=dense, hybrid=fusion(chunks, [lexical, dense]))
        for method, ranked in rankings.items():
            hits = gold[query['id']].intersection(ranked[:5])
            gold_paths = {chunks[i]['path'] for i in gold[query['id']]}
            file_hit = bool(gold_paths.intersection(chunks[i]['path'] for i in ranked[:5]))
            reciprocal = next((1/r for r, i in enumerate(ranked[:10], 1) if i in gold[query['id']]), 0)
            rows.append({'id': query['id'], 'split': query['split'], 'critical': query.get('critical', False),
                         'method': method, 'recall_at_5': len(hits)/len(gold[query['id']]),
                         'hit_at_5': float(bool(hits)), 'mrr_at_10': reciprocal,
                         'file_hit_at_5': float(file_hit),
                         'gold_ids': [chunks[i]['id'] for i in sorted(gold[query['id']])],
                         'ranking': [chunks[i]['id'] for i in ranked[:10]]})
    summaries = []
    for split in ('dev', 'test'):
        baseline = [r for r in rows if r['split'] == split and r['method'] == 'lexical']
        if not baseline:
            continue
        for method in sorted({r['method'] for r in rows}):
            selected = [r for r in rows if r['split'] == split and r['method'] == method]
            summary = {'split': split, 'method': method, 'queries': len(selected)}
            for metric in ('recall_at_5', 'hit_at_5', 'mrr_at_10', 'file_hit_at_5'):
                summary[metric] = statistics.mean(r[metric] for r in selected)
            critical = [r['hit_at_5'] for r in selected if r['critical']]
            summary['critical_hit_at_5'] = statistics.mean(critical) if critical else None
            summary['eligible_for_selection'] = split == 'dev'
            summary['passes_quality'] = (summary['recall_at_5'] >= statistics.mean(r['recall_at_5'] for r in baseline)
                and summary['hit_at_5'] >= .9 and summary['critical_hit_at_5'] == 1.0
                and summary['mrr_at_10'] >= statistics.mean(r['mrr_at_10'] for r in baseline) - .02)
            summaries.append(summary)
    return {'schema': SCHEMA, 'corpus_hash': corpus['corpus_hash'], 'summaries': summaries,
            'per_query': rows, 'run': index.get('metadata') if index else None,
            'selection_split': 'dev',
            'selection': 'No automatic winner. Only dev metrics may select candidates; test is confirmation after selection is frozen.'}


def config_identity(config):
    if not isinstance(config.get('id'), str) or not config['id'] or not isinstance(config.get('revision'), str) or not config['revision']:
        raise ValueError('Explicit model id and revision required')
    if not re.fullmatch(r'[0-9a-fA-F]{40}', config['revision']):
        raise ValueError('Model revision must be an immutable 40-hex commit')
    if config.get('trust_remote_code', False):
        raise ValueError('Remote code loading is disabled')
    if config.get('dimensions') is not None:
        dimension = config['dimensions']
        if isinstance(dimension, bool) or not isinstance(dimension, int) or dimension <= 0:
            raise ValueError('Reduced dimensions must be a positive integer')
        if config.get('matryoshka') is not True:
            raise ValueError('Reduced dimensions require explicit Matryoshka support')
    if config.get('quantization', 'none') not in ('none', 'dynamic-int8'):
        raise ValueError('Unsupported quantization experiment')
    backend = config.get('backend', 'sentence-transformers')
    if backend not in ('sentence-transformers', 'onnx'):
        raise ValueError('Unsupported embedding backend')
    if backend == 'onnx':
        onnx_file = config.get('onnx_file')
        if (not isinstance(onnx_file, str) or '\\' in onnx_file or ':' in onnx_file
                or PurePosixPath(onnx_file).is_absolute() or '..' in PurePosixPath(onnx_file).parts
                or str(PurePosixPath(onnx_file)) != onnx_file or not onnx_file.endswith('.onnx')):
            raise ValueError('Explicit canonical relative ONNX filename required')
        if config.get('pooling') not in ('mean', 'cls'):
            raise ValueError('Explicit ONNX pooling required')
        if config.get('parameter_quantization', 'none') not in ('none', 'int8', 'q4'):
            raise ValueError('Unsupported ONNX parameter quantization label')
        if config.get('quantization', 'none') != 'none':
            raise ValueError('Dynamic Torch quantization is incompatible with ONNX')
    identity = {k: config.get(k) for k in ('id', 'revision', 'query_prefix', 'document_prefix', 'dimensions', 'matryoshka')}
    identity['quantization'] = config.get('quantization', 'none')
    identity.update(backend=backend, onnx_file=config.get('onnx_file'), pooling=config.get('pooling'),
                    parameter_quantization=config.get('parameter_quantization', 'none'))
    adapter = config.get('loader_adapter')
    if adapter is not None:
        if adapter != 'coderank-cls-v1' or config['id'] != 'nomic-ai/CodeRankEmbed' or backend != 'sentence-transformers':
            raise ValueError('Unsupported model loader adapter')
        identity['loader_adapter'] = adapter
    return identity


def load_index(path, corpus, config=None):
    validate_corpus(corpus)
    rows = [json.loads(line) for line in Path(path).read_text(encoding='utf-8').splitlines()]
    if not rows or rows[0].get('type') != 'metadata':
        raise ValueError('Missing index metadata')
    metadata = rows[0]
    if metadata.get('schema') != SCHEMA or metadata.get('corpus_hash') != corpus['corpus_hash'] or metadata.get('source_head') != corpus.get('source_head'):
        raise ValueError('Index freshness/corpus mismatch')
    if config is not None and metadata.get('model') != config_identity(config):
        raise ValueError('Index model mismatch')
    if (isinstance(metadata.get('dimension'), bool) or not isinstance(metadata.get('dimension'), int)
            or metadata['dimension'] <= 0):
        raise ValueError('Malformed index dimension')
    documents = [r for r in rows[1:] if r.get('type') == 'chunk']
    if len(documents) != len(corpus['chunks']):
        raise ValueError('Index chunk count mismatch')
    vectors = []
    for stored, chunk in zip(documents, corpus['chunks']):
        if stored.get('chunk') != chunk:
            raise ValueError('Index source chunk mismatch')
        vectors.append(vector(stored.get('vector'), metadata.get('dimension')))
    queries = {}
    for row in rows[1:]:
        if row.get('type') not in ('chunk', 'query'):
            raise ValueError('Unexpected index row type')
        if row.get('type') == 'query':
            if row['id'] in queries:
                raise ValueError('Duplicate index query')
            queries[row['id']] = {'text': row['text'], 'vector': vector(row.get('vector'), metadata['dimension'])}
    return {'metadata': metadata, 'vectors': vectors, 'queries': queries}


def directory_bytes(path):
    return sum(p.stat().st_size for p in Path(path).rglob('*') if p.is_file())


def load_model(config, cache):
    config_identity(config)
    if not Path(cache).is_absolute():
        raise ValueError('Model cache must be an explicit absolute path')
    os.environ.update(HF_HOME=str(cache), HF_HUB_CACHE=str(Path(cache)/'hub'),
                      OMP_NUM_THREADS='2', MKL_NUM_THREADS='2', TOKENIZERS_PARALLELISM='false')
    if config.get('backend') == 'onnx':
        return OnnxEncoder(config, cache), 0
    # Imports occur only for explicit encoding or novel-query search.
    import torch
    from sentence_transformers import SentenceTransformer
    torch.set_num_threads(2)
    torch.set_num_interop_threads(2)
    if config.get('loader_adapter') == 'coderank-cls-v1':
        # The pinned checkpoint's sentence_bert_config incorrectly passes the save-only
        # safe_serialization flag to model loading. Build its declared CLS pipeline directly;
        # preserve original pinned weights/config and keep remote code disabled.
        from sentence_transformers import models
        common = {'revision': config['revision'], 'cache_dir': str(cache), 'trust_remote_code': False}
        transformer = models.Transformer(config['id'],
            model_kwargs=dict(common, use_safetensors=True, add_pooling_layer=False),
            processor_kwargs=common, config_kwargs=common, max_seq_length=256)
        model = SentenceTransformer(modules=[transformer, models.Pooling(768, pooling_mode='cls')], device='cpu')
    else:
        model = SentenceTransformer(config['id'], revision=config['revision'], device='cpu',
                                    cache_folder=str(cache), trust_remote_code=False,
                                    model_kwargs={'use_safetensors': True})
    model.max_seq_length = 256
    quantized_layers = 0
    if config.get('quantization', 'none') == 'dynamic-int8':
        model[0].auto_model = torch.ao.quantization.quantize_dynamic(
            model[0].auto_model, {torch.nn.Linear}, dtype=torch.qint8, inplace=False)
        quantized_layers = sum(isinstance(module, torch.ao.nn.quantized.dynamic.Linear)
                               for module in model.modules())
        if not quantized_layers:
            raise ValueError('Dynamic INT8 experiment produced zero quantized layers')
    return model, quantized_layers


def pool_hidden(hidden, masks, pooling):
    """Small portable pooling reference, also used by the ONNX backend."""
    if len(hidden) != len(masks):
        raise ValueError('ONNX output batch mismatch')
    pooled = []
    for document, mask in zip(hidden, masks):
        if not document:
            raise ValueError('Empty ONNX output')
        if isinstance(document[0], (int, float)):
            pooled.append(vector(document))
        elif pooling == 'cls':
            pooled.append(vector(document[0]))
        else:
            if len(document) != len(mask) or not any(mask):
                raise ValueError('ONNX attention mask mismatch/empty')
            summed = [sum(row[d] * weight for row, weight in zip(document, mask))
                      / sum(mask) for d in range(len(document[0]))]
            pooled.append(vector(summed))
    return pooled


class OnnxEncoder:
    def __init__(self, config, cache):
        import onnxruntime as ort
        from huggingface_hub import hf_hub_download
        from tokenizers import Tokenizer
        self.config = config
        def download(filename):
            return hf_hub_download(repo_id=config['id'], revision=config['revision'],
                                   filename=filename, cache_dir=str(cache))
        self.tokenizer = Tokenizer.from_file(download('tokenizer.json'))
        self.tokenizer.enable_truncation(max_length=256)
        # B106: public tokenizer padding symbol, not a password or credential.
        self.tokenizer.enable_padding(pad_id=self.tokenizer.token_to_id('[PAD]') or 0, pad_token='[PAD]')  # nosec B106
        self.weights_path = Path(download(config['onnx_file']))
        options = ort.SessionOptions()
        options.intra_op_num_threads = 2
        options.inter_op_num_threads = 2
        # Session loading fails if external data is missing; no further remote files are fetched.
        self.session = ort.InferenceSession(str(self.weights_path), sess_options=options,
                                           providers=['CPUExecutionProvider'])
        if self.session.get_providers() != ['CPUExecutionProvider']:
            raise ValueError('ONNX execution must use CPUExecutionProvider exclusively')

    def count_truncated(self, texts):
        self.tokenizer.no_truncation()
        try:
            return sum(len(self.tokenizer.encode(text).ids) > 256 for text in texts)
        finally:
            self.tokenizer.enable_truncation(max_length=256)

    def encode(self, texts):
        import numpy as np
        output = []
        for start in range(0, len(texts), 8):
            encoded = self.tokenizer.encode_batch(texts[start:start+8])
            fields = {'input_ids': [e.ids for e in encoded],
                      'attention_mask': [e.attention_mask for e in encoded],
                      'token_type_ids': [e.type_ids for e in encoded]}
            feeds = {}
            for item in self.session.get_inputs():
                if item.name not in fields or item.type != 'tensor(int64)':
                    raise ValueError(f'Unsupported ONNX input: {item.name} {item.type}')
                feeds[item.name] = np.asarray(fields[item.name], dtype=np.int64)
            names = [item.name for item in self.session.get_outputs()]
            preferred = next((name for name in ('sentence_embedding', 'last_hidden_state', 'token_embeddings') if name in names), names[0])
            hidden = self.session.run([preferred], feeds)[0]
            if hidden.ndim not in (2, 3):
                raise ValueError('Unsupported ONNX embedding output rank')
            output.extend(pool_hidden(hidden.tolist(), fields['attention_mask'], self.config['pooling']))
        return output


def truncated_count(model, texts):
    if isinstance(model, OnnxEncoder):
        return model.count_truncated(texts)
    return sum(len(model.tokenizer(text, truncation=False)['input_ids']) > 256 for text in texts)


def embeddings(model, config, texts, prefix):
    texts = [prefix+t for t in texts]
    if isinstance(model, OnnxEncoder):
        encoded = model.encode(texts)
    else:
        encoded = model.encode(texts, batch_size=8, device='cpu',
                               normalize_embeddings=True, show_progress_bar=False, prompt='').tolist()
    dimension = config.get('dimensions')
    if dimension and dimension > len(encoded[0]):
        raise ValueError('Invalid reduced dimension')
    return [vector(v[:dimension] if dimension else v) for v in encoded]


def encode(corpus, qrels, config, output, cache):
    identity = config_identity(config)
    gold_targets(corpus, qrels)
    stop = threading.Event()
    peak = [None]
    try:
        import psutil
        process = psutil.Process()
        def sample():
            while not stop.wait(.025):
                peak[0] = max(peak[0] or 0, process.memory_info().rss)
        sampler = threading.Thread(target=sample, daemon=True)
        sampler.start()
    except ImportError:
        sampler = None
    try:
        started = time.perf_counter()
        model, quantized_layers = load_model(config, cache)
        load_seconds = time.perf_counter() - started
        Path(output).parent.mkdir(parents=True, exist_ok=True)
        if isinstance(model, OnnxEncoder):
            state_path = model.weights_path
            with state_path.open('rb') as stream:
                weight_hash = hashlib.file_digest(stream, 'sha256').hexdigest()
        else:
            state_path = Path(output).parent / 'weights.pt'
            if state_path.exists():
                raise ValueError('Run directory already contains weights.pt; use an isolated model run directory')
            import torch
            # Sizing artifact only: the harness never reloads this pickle/state dictionary.
            torch.save(model.state_dict(), state_path)
            weight_hash = None
        serialized_state_bytes = state_path.stat().st_size
        texts = [c['text'] for c in corpus['chunks']]
        document_prefix = config.get('document_prefix') or ''
        query_prefix = config.get('query_prefix') or ''
        truncated = truncated_count(model, [document_prefix+t for t in texts])
        started = time.perf_counter()
        vectors = embeddings(model, config, texts, document_prefix)
        index_seconds = time.perf_counter() - started
        queries, latencies = [], []
        for q in qrels['queries']:
            query_vector = embeddings(model, config, [q['query']], query_prefix)[0]
            queries.append({'type': 'query', 'id': q['id'], 'text': q['query'], 'vector': query_vector})
            for _ in range(5):
                started = time.perf_counter()
                warmed = embeddings(model, config, [q['query']], query_prefix)[0]
                ranked = dense_rank(corpus['chunks'], vectors, warmed)
                fusion(corpus['chunks'], [bm25(corpus['chunks'], q['query']), ranked])
                latencies.append(1000*(time.perf_counter()-started))
        latencies.sort()
        metadata = {'type': 'metadata', 'schema': SCHEMA, 'corpus_hash': corpus['corpus_hash'],
                    'source_head': corpus.get('source_head'), 'model': identity, 'dimension': len(vectors[0]),
                    'device': 'cpu', 'threads': 2, 'max_seq_length': 256, 'batch_size': 8,
                    'load_seconds': load_seconds, 'index_seconds': index_seconds,
                    'quantized_layers': quantized_layers, 'serialized_state_bytes': serialized_state_bytes,
                    'weights_sha256': weight_hash,
                    'warm_query_p50_ms': statistics.median(latencies),
                    'warm_query_p95_ms': latencies[math.ceil(.95*len(latencies))-1],
                    'query_timing_scope': 'encode + dense + BM25 + fixed RRF; five warm repetitions/query',
                    'truncated_documents': truncated, 'peak_rss_bytes': peak[0],
                    'cache_total_bytes': directory_bytes(cache)}
        Path(output).parent.mkdir(parents=True, exist_ok=True)
        with Path(output).open('w', encoding='utf-8') as stream:
            for row in [metadata] + [{'type': 'chunk', 'chunk': c, 'vector': v} for c, v in zip(corpus['chunks'], vectors)] + queries:
                stream.write(json.dumps(row, allow_nan=False)+'\n')
        metadata['artifact_bytes'] = Path(output).stat().st_size
        write_json(str(output)+'.run.json', metadata)
        return metadata
    finally:
        stop.set()
        if sampler:
            sampler.join()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('prepare')
    p.add_argument('--repo', required=True); p.add_argument('--manifest', required=True)
    p.add_argument('--output', required=True)
    p.add_argument('--tokenizer', help='Offline common tokenizer.json for token-bounded chunks')
    p.add_argument('--max-tokens', type=int, default=224)
    p.add_argument('--overlap-tokens', type=int, default=32)
    for name in ('encode', 'evaluate', 'search'):
        p = sub.add_parser(name)
        p.add_argument('--corpus', required=True)
        p.add_argument('--repo', help='Recheck every allowlisted source against the prepared corpus')
        p.add_argument('--index', required=name == 'search')
        p.add_argument('--config')
        if name != 'search':
            p.add_argument('--qrels', required=True)
            p.add_argument('--output', required=True)
        if name == 'encode':
            p.add_argument('--cache', required=True)
        if name == 'search':
            group = p.add_mutually_exclusive_group(required=True)
            group.add_argument('--query-id')
            group.add_argument('--text', help='Encode a novel query with the pinned model configuration')
            p.add_argument('--cache')
    p = sub.add_parser('report'); p.add_argument('reports', nargs='+')
    args = parser.parse_args()
    if args.command == 'prepare':
        corpus = prepare(args.repo, read_json(args.manifest), tokenizer_path=args.tokenizer,
                         max_tokens=args.max_tokens, overlap_tokens=args.overlap_tokens)
        write_json(args.output, corpus)
        print(json.dumps({'corpus_hash': corpus['corpus_hash'], 'chunks': len(corpus['chunks'])}))
    elif args.command == 'report':
        for report in args.reports:
            value = read_json(report)
            print(json.dumps({'report': report, 'summaries': value['summaries'], 'run': value.get('run')}))
    else:
        corpus = validate_corpus(read_json(args.corpus), args.repo)
        config = read_json(args.config) if args.config else None
        if args.command == 'encode':
            if not config:
                parser.error('encode requires --config')
            print(json.dumps(encode(corpus, read_json(args.qrels), config, args.output, args.cache)))
        else:
            index = load_index(args.index, corpus, config) if args.index else None
            if args.command == 'evaluate':
                result = evaluate(corpus, read_json(args.qrels), index)
                write_json(args.output, result)
                print(json.dumps(result['summaries']))
            else:
                if args.text is not None:
                    if not config or not args.cache:
                        parser.error('search --text requires --config and --cache')
                    model, _ = load_model(config, args.cache)
                    query_vector = embeddings(model, config, [args.text], config.get('query_prefix') or '')[0]
                else:
                    query_vector = index['queries'][args.query_id]['vector']
                ranked = dense_rank(corpus['chunks'], index['vectors'], query_vector)
                print(json.dumps([corpus['chunks'][i] for i in ranked[:10]], indent=2))


if __name__ == '__main__':
    main()

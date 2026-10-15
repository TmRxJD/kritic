# Local semantic map — research experiment

This is an optional evidence-retrieval prototype, not an acceptance checker. It does not change
Kritic's deterministic default, call a cloud service, execute retrieved text or approve a contract.
The benchmark uses Python to compare established embedding runtimes; Kritic's production server
remains Rust. A production integration is conditional on the measured results.

## Corpus and evaluation

`bench/semantic-corpus.json` explicitly selects 30 files from Kritic source, tests, documentation
and its minimal profile at the recorded source revision. No workspace discovery or conversation
indexing occurs. The map stores source paths, line ranges, content hashes, corpus identity,
model revision and vectors. Rebuild it when source bytes or embedding configuration change.

`bench/semantic-queries.json` contains 12 development queries and 24 separate confirmation
queries. Labels are manually specified source anchors, not generated answers. Queries ask for
evidence locations using natural language, including stale evidence, vacuous acceptance,
network boundaries, CPU-only operation and configuration. No candidate is fine-tuned.

The initial word-based pilot was discarded for model selection: 312 of 449 chunks exceeded
the first model's 256-token limit. Rust code has many more tokens per word than ordinary prose.
That preprocessing issue was identified from coverage, before selecting any model.

The confirmation matrix uses one common, pinned MiniLM tokenizer to make contiguous source
spans of at most 224 tokens with 32-token overlap. This leaves room for model-specific prefixes
and special tokens within the common 256-token model limit. All candidates use these same
chunks, CPU execution, two compute threads and batches of eight. The run still records any
truncation under each candidate's own tokenizer. Each model uses its own
document/query instructions, pinned revision and supported pooling. Remote model code is
disabled. The primary runtime uses official pinned ONNX artifacts and the CPU execution
provider; the optional Sentence Transformers path requires safetensors.

Compare BM25, dense cosine search and a fixed reciprocal-rank fusion of the two (constant 60).
Report Hit@5, anchor-chunk Recall@5 and MRR@10 for each split, plus Hit@5 on explicitly marked
critical questions. Overlapping chunks containing the same anchor are all labelled relevant,
so chunk recall is stricter than merely locating the source function.
An auxiliary file-level Hit@5 distinguishes navigation to the right file from locating the
labelled evidence. It was added as a diagnostic after the pilot; it does not replace the
predeclared quality gate or justify selecting a model that fails that gate.

The quality bar is declared before model runs: Hit@5 at least 90%, all labelled critical
questions found in the first five results, no loss in mean Recall@5 versus BM25, and MRR@10
no more than 0.02 below BM25. Select configurations on development results, then use the
separate questions for confirmation. If none passes, report that outcome; do not lower the
threshold after seeing results or declare the least-bad model sufficient.

Resource targets for this experiment are zero model VRAM, at most 1.5 GiB measured process
RSS and warm end-to-end query p95 below 500 ms on the recorded host. These are experiment
targets, not minimum specifications for every laptop. Warm timings include query embedding,
full-corpus dense search, BM25 and fusion. The load measurement includes runtime imports,
model loading and any needed downloads; it is not an isolated cold-start measurement. Indexing
is timed separately. Python/ML runtime installation size is distinct from model weight size and index
size. Existing shared runtimes do not count as free dependencies for new users.

## Candidate ladder

- `sentence-transformers/paraphrase-MiniLM-L3-v2`: small lower-bound candidate.
- `sentence-transformers/all-MiniLM-L6-v2`: compact general text baseline.
- `BAAI/bge-small-en-v1.5`: compact retrieval-oriented candidate.
- `nomic-ai/nomic-embed-text-v1.5`: larger text candidate; reduced vector dimensions are valid
  experiments because this model supports Matryoshka representations.
- `nomic-ai/CodeRankEmbed`: Nomic's smaller code-retrieval candidate, separate from its 7B
  `nomic-embed-code` model.

Official INT8 ONNX variants or CPU dynamic INT8 weights may be compared separately. Shortening an embedding vector reduces
map storage, not the model's parameter count. Arbitrary vector truncation is not a supported
compression method for candidates without explicit Matryoshka support.

## Limits

This is a small, single-repository, developer-labelled benchmark. Its relevance labels are
incomplete: an unlabelled passage may still be useful. Similarity does not establish that a
requirement is enforced, a test ran, or a claim is true. The test split is a confirmation set
for this experiment, not a permanent untouched benchmark once results have been inspected.
No open-world abstention or multilingual quality claim follows from these questions.

Retrieved material must retain its source and freshness information. Treat it as untrusted
candidate evidence; validate the underlying code, commands and current results independently.
Exact identifiers, source dependencies and lexical search remain necessary when semantic
ranking misses relevant material.

## Measured results (2026-10-15)

No candidate qualified on the 12 development questions. The 24 confirmation questions below
are comparative results, not a basis for selecting a winner after inspecting the test set.
All inference used two CPU threads on an Intel Core i7-9700K, with zero model VRAM.
Sizes use decimal MB; memory is sampled process RSS, not total system commit.

| Encoder | Weights MB | Peak RSS MB | Warm p95 ms | Development hybrid Hit@5 | Confirmation hybrid Hit@5 |
|---|---:|---:|---:|---:|---:|
| MiniLM-L3, 8-bit ONNX | 17.5 | 286 | 65 | 16.7% | 25.0% |
| MiniLM-L3, float ONNX | 69.0 | 325 | 69 | 16.7% | 25.0% |
| MiniLM-L6, float ONNX | 90.4 | 348 | 103 | 33.3% | 45.8% |
| BGE-small-en-v1.5, float ONNX | 133.1 | 389 | 67 | 33.3% | 37.5% |
| Nomic embed-text-v1.5, float ONNX | 547.3 | 1222 | 211 | 25.0% | 50.0% |
| BM25, no encoder | — | — | — | 25.0% | 54.2% |

The 8-bit L3 variant preserved its float counterpart's aggregate scores, but both missed
the quality bar. Fixed equal-weight reciprocal-rank fusion underperformed BM25 on the
confirmation set. There is no evidence here to replace lexical search or enable automatic
semantic evidence selection. A local map remains an experimental navigation aid whose
results require source inspection.

The frozen corpus contains 649 token-aware chunks from 30 files at commit
`42362069a77e458dddc70eb4e2c226f80bcbcae9`; its SHA-256 is
`93f67a5532cd35766fbde6ed8631eb7f16b2e0be852cb662a4310b7f938dc869`.
All completed runs had zero truncated passages. An earlier word-based pilot was discarded
because 312 of 449 passages exceeded the encoder limit. Relevance labels identify exact
source anchors and are incomplete; file-level hits are diagnostic only. These numbers do
not establish general code-search quality.

CodeRankEmbed has no quality result. Its saved loader config initially raised
`TypeError: NomicBertModel.__init__() got an unexpected keyword argument 'safe_serialization'`.
The explicit CLS adapter below corrected loading, but the resource guard stopped encoding
at `commit_reserve_below_8GiB`. Shared-model services were left running. Its size and accuracy
must not be inferred from the completed candidates.

## Reproduction

Use an isolated environment and explicit cache/output directories. These Windows examples use
E: to keep models, caches and generated data off the system drive. ONNX runs need no Torch,
CUDA, model-serving daemon or cloud inference credentials.

```powershell
python -m venv E:/AI/work/kritic-semantic/venv
E:/AI/work/kritic-semantic/venv/Scripts/python.exe -m pip install -r bench/requirements-semantic.txt
E:/AI/work/kritic-semantic/venv/Scripts/python.exe -m unittest discover -s bench -p test_semantic_map.py
```

Select a single pinned entry from `bench/semantic-models.json` and save it as `model.json` in
your output directory. Download that revision's `tokenizer.json` with `huggingface_hub` and
pass its local path explicitly. The recorded experiment freezes the 30-file source corpus at
the manifest's revision; use a separate checkout/snapshot of that revision rather than editing
files while the benchmark runs.

```text
python bench/semantic_map.py prepare --repo <source-snapshot> --manifest bench/semantic-corpus.json --tokenizer <local-tokenizer.json> --max-tokens 224 --overlap-tokens 32 --output <output>/corpus.json
python bench/semantic_map.py encode --repo <source-snapshot> --corpus <output>/corpus.json --qrels bench/semantic-queries.json --config <output>/model.json --cache <absolute-cache-directory> --output <output>/candidate/index.jsonl
python bench/semantic_map.py evaluate --repo <source-snapshot> --corpus <output>/corpus.json --qrels bench/semantic-queries.json --config <output>/model.json --index <output>/candidate/index.jsonl --output <output>/candidate/report.json
python bench/semantic_map.py search --repo <source-snapshot> --corpus <output>/corpus.json --config <output>/model.json --index <output>/candidate/index.jsonl --cache <absolute-cache-directory> --text "Where is stale acceptance evidence invalidated?"
```

For model-free BM25 evaluation, omit `--index` and `--config` from `evaluate`. Saved benchmark
query search can use `--query-id` instead of `--text` without loading an encoder. For an offline
novel query after the artifacts are cached, set `HF_HUB_OFFLINE=1`. Missing artifacts fail;
there is no cloud-inference fallback. Loading the CPU model for a new process has a separate
startup cost; warm-query measurements assume it remains loaded.

CodeRankEmbed is optional and requires the additional measured Sentence Transformers/CPU
Torch stack. Its pinned saved module config includes an invalid load argument,
`safe_serialization`; the explicit `coderank-cls-v1` adapter bypasses that save-only argument
by reconstructing the checkpoint's declared 768-dimensional CLS pooling pipeline. It keeps
the pinned original weights, disables remote code and records the adapter in index identity.
The ONNX-only requirements file does not install this optional stack.

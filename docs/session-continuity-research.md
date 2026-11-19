# Session continuity: evidence and design constraints

Reviewed 2026-11-19. This is a targeted engineering review, not a systematic
literature review or a benchmark of the user's installed models.

## What the research supports

- [LLMs Get Lost In Multi-Turn Conversation](https://arxiv.org/abs/2505.06120)
  measured lower performance and increased unreliability in simulated multi-turn,
  progressively specified tasks. Its reported average reduction is scoped to those
  tasks and models. It is not evidence that every agent becomes a fixed percentage
  worse at a particular turn count. Avoid carrying early unsupported assumptions
  forward as established facts.
- [Lost in the Middle](https://arxiv.org/abs/2307.03172) found sensitivity to where
  relevant information appears in long inputs on retrieval/question-answering tasks.
  It motivates explicit high-priority context, but does not establish an automatic
  session-reset threshold for current coding agents.
- [Chroma's Context Rot report](https://www.trychroma.com/research/context-rot)
  varied input length on controlled tasks across 18 models and observed nonuniform
  reliability. The authors identify limited task coverage and do not explain the
  underlying mechanism. Model/task-specific measurement is preferable to a universal
  occupancy cliff.
- [HumanLayer's context-engineering practice](https://www.humanlayer.dev/blog/advanced-context-engineering)
  recommends frequent intentional compaction and a roughly 40–60% utilization
  range, depending on complexity. This is a practitioner workflow and heuristic,
  not a scientifically calibrated universal “dumb zone.”
- [Anthropic's context-engineering guidance](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
  describes compaction, persistent notes and isolated subagents. It warns that
  aggressive compaction can lose important details and recommends optimizing
  recall before reducing noise. Compaction can already reinitialize a model's
  working context; a long history file does not by itself prove all its tokens
  remain in the model's current input.
- [Anthropic's long-running harness work](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
  emphasizes explicit progress artifacts, current repository state, feature
  obligations and real checks between sessions. Its examples are engineering
  experience, not proof that an arbitrary handoff is complete.
- [LongMemEval](https://arxiv.org/abs/2410.10813) distinguishes extraction,
  multi-session reasoning, temporal reasoning, knowledge updates and abstention.
  The authors' [2026 V2 benchmark](https://github.com/xiaowu0162/LongMemEval-V2)
  additionally emphasizes dynamic state, workflow knowledge, environment gotchas
  and premise awareness. These are useful regression categories for stale
  fixtures and repeated user corrections; Kritic has not been benchmarked on them.

Search routes included focused title/entity searches and primary author/vendor
pages, followed through the original LongMemEval repository to its newer version.
Nonprimary summaries, social-media numerical claims and news results were excluded.
An initially guessed LongMemEval repository URL returned 404; the paper and author
repository above were subsequently verified.

## Product policy derived from that evidence

These are design decisions, not measured guarantees:

1. Keep historical transcript size, observed current-context tokens and cumulative
   session activity as separate metrics. Missing telemetry is unknown, not zero.
   Set configurable operational limits and label them policy thresholds rather
   than claims that a model has become “dumb.”
2. Trigger preparation during the turn but perform transfer only after the observed
   turn boundary. Never interrupt an active edit, silently discard work, or create
   duplicate writer agents after an uncertain launch response.
3. Preserve original history. Curate a new context view; never delete the source
   transcript or use compaction/forking as proof that a genuinely fresh session
   started. A host controller must report actual creation and acknowledgement.
4. Pin the latest goal, unresolved obligations, exact workspace/head/dirty files,
   decisions, failed attempts, actual test outcomes, service ownership and next
   step. Every material fact must cite an observed event or verified artifact.
5. Inventory every user event. Corrections and overarching requests cannot be
   silently dropped by relevance ranking. Link superseded instructions explicitly.
   Retain the corrective instruction and its confirmed solution, rather than an
   emotional label or speculative personality profile. Unresolved solutions remain
   unresolved. This project ledger is separate from provider-owned personal memory.
6. Fixture selections need live authority/revision/hash evidence. Recheck on resume;
   historical passing tests and filenames cannot select the current fixture.
   Integrate the existing provenance selector rather than inventing a second source
   of authority.
7. Kev may classify bounded cited chunks for relevance or potential contradictions.
   It never writes a handoff, invents facts, grants transfer readiness, or overrides
   protected user corrections. Missing/invalid advice keeps deterministic coverage.
8. A handoff critic can verify declared coverage, hashes, references and state. It
   cannot prove semantic completeness from an incomplete transcript or a set of
   fields supplied by the same agent. Unknown capture coverage prevents unattended
   transfer; a separate reviewer and a successor acknowledgement add independent
   checks.

## Required negative controls

Omitted user correction; obsolete fixture with a newer filename; changed canonical
hash; truncated/missing transcript events; incomplete task inventory; unresolved
contradiction; a new user prompt after validation; dirty workspace changes after
snapshot; absent host capability; fork misreported as fresh context; ambiguous
launch timeout; wrong handoff hash; missing successor acknowledgement; restart
during transfer; and accidental duplicate worktree writers.

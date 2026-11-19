# Session and handoff critics

Opt-in, model-free checks for **declared, pinned evidence**, not a diagnosis of
impairment or proof that a summary is semantically complete.
[Research and design limits](session-continuity-research.md).

Enable `capabilities.sessions=true` or `KRITIC_CAP_SESSIONS=true` in a matching
source build. `kritic session --schema` prints the authoritative session/handoff
schemas. The published test.13 binary predates these critics. Automatic lifecycle
enrollment is separate; enabling review tools does not restart anything.

Keep private exports/cases/reports in permanent `.kritic/session-state/` storage,
ignored by Git. Defaults are `session.json` and `handoff.json` there. Never use
disposable scratch for a real handoff, partial work or original history.

## Required session evidence

- Actual host/session/task, current goal, scope, overarching requests and explicit
  task state (`active`, `complete`, `unknown`). Active tasks need an unfinished
  obligation; completed tasks do not spawn another task because a limit was hit.
- Capture path/hash/format/completeness/gaps. Supports native Codex/Claude JSONL,
  a Hermes export with `session_id`/`messages`, or normalized JSONL records with
  `session_id`, `id`, `role`, `text`. Native event IDs use source line numbers.
  A caller's completeness flag is an assertion, not proof the host exported all
  history. Summaries and compacted/forked context cannot prove complete capture.
- Exact Git revision and hashes for **every** dirty/untracked file, plus explicit
  deletion tombstones. Work is preserved; nothing is reset or recreated.
- Cited fact IDs, kinds and exact event quotes. Requirements/corrections must cite
  user events. Solutions are unresolved/observed/user_confirmed; observations need
  pinned artifacts, user confirmation an actual user event. Supersession requires
  a later user event; earlier instructions remain visible as superseded history.
- Fixtures pin selected/current payloads, revisions, and an existing canonical
  JSON binding file/hash/pointer. A `.kev` manifest can use `/authority`, containing
  `path`, `sha256`, `revision`. Current selection must match that live binding.
  Intentional historical use needs a reason quoted from a user authorization.
  This supplements `kev-tools.staleness_evaluate`; it does not prove pipeline reachability.

## Use

`session_critic(base_dir, case_path?)` reports measured source bytes/user turns and
current-input token usage/window when available. Cumulative usage is not current
context. Limits are operational policy; unknown telemetry is null. Due rotation
explicitly allows the current turn to finish.

`handoff_critic(base_dir, case_path?)` requires a hashed session case, source session,
every captured user event ID, every declared fact/fixture ID, next step, uncertainties
and concrete successor continuity questions. Missing/unknown/duplicate IDs block.
Changed artifacts or workspace cannot be blessed by an older passing report.

Optional `session_critic(curate=true, use_kev=true)` uses the existing lossless archive
and bounded typed relevance advice. All user messages, noncontext facts and solution
observations are protected. Original content remains retrievable. Its token counts
are character estimates, not provider telemetry; model failure is explicit.

Handoff status is covered/blocked/unknown. Covered is scoped evidence, never semantic
approval. The full persisted report includes curated context; normal MCP output
returns its reference. The explicit fresh-context **byte** budget blocks overflow
instead of silently dropping protected information. CLI:
`kritic session --base-dir <repo> --case <file> --handoff --output <new report path>`.
Different existing reports are never overwritten.

A controller still needs observed turn completion, a live user/workspace generation
recheck, independent review, actual fresh-session creation and successor acknowledgement.
Forking retains history. Unknown controller/capture capability is a setup gap, not
fake seamless continuation; the critic itself never grants automatic transfer.
Preserve history and recovery records; reconcile uncertain launches before retrying.

The [worktree coordination transfer protocol](../integrations/coordination/README.md#resume-the-same-worktree-in-a-fresh-session)
connects this evidence to durable ownership. The successor takes over the same
existing worktree, task and scopes after a hash-checked acknowledgement. The source
stops writing while transfer is pending and remains fenced after acceptance.
Runtime exit does not release the successor's claim. No checkout, reset, merge or
deletion occurs during transfer. A launcher must use the actual fresh-session ID,
not a pending desktop card, and reconcile uncertain startup before launching again.

Limits fail explicitly: 1 MiB case, 64 MB capture/20,000 messages, 20 MB per evidence
file, 256 dirty paths/facts, 64 fixtures, 4 KiB–1 MiB fresh-context budget, and bounded
five-second Git inspection. Synthetic fixtures are not evidence of real-user recall.

# Changelog

## 0.5.0 - 2026-11-20

- Add explicit user-authorized recovery when an owner hits a token limit, crashes or cannot cooperate. Inspect an exact assignment, then accept its hash-bound plan as the actual successor without source release or a fresh source handoff.
- Preserve task/scopes, full staged identity and dirty/untracked work; reject stale generation, activity, Git state, conflicts and wrong-session plans. Atomically fence the old writer and invalidate pending transfers.
- Expose recovery in MCP, strict native-command bootstrap and host hooks before declaration. Record the authorization and unverified context; no timeout-based eviction or automatic merge occurs.
- Keep recovery plans free of floating-point values and separately hash the exact stored declaration. A real MCP round-trip exposed timestamp rounding; acceptance retains original metadata and now survives transport without weakening hash checks.

## 0.4.0 - 2026-11-19

- Add explicit same-worktree ownership transfer phases for a prepared, bound and accepted session successor; preserve task/scopes, Git state and dirty evidence, and fence previous writers.
- Keep editing ownership durable across session-end events. Cancellation preserves the original assignment; unavailable peers cannot silently discard claims.
- Expose transfer schemas on demand. Automatic session launching remains a separate host-controller integration; a transfer never implies a merge or cleanup.

## 0.3.0 - 2026-11-19

- Add opt-in `worktree_evaluate` actions for shared ownership declarations, scope status/history and read-only worktree intent review.
- Bridge to one cross-harness coordinator with transactional worktree/scope reservations and preserved intent history.
- Compare branch deltas to an explicit main ref, preserve dirty and unavailable worktrees, and isolate only reviewed text hunks without merging or cleaning anything.
- Add optional bounded Kev intent advice with validated typed labels and explicit unreviewed counts.
- Document hook identity and shell coverage, including Codex shared subagent session IDs and Hermes blocking capability probes.

## 0.2.0 - 2026-11-19

- Add `staleness_evaluate` with an on-demand strict provenance manifest schema and persistent cross-session workflow.
- Classify declared artifact versions using canonical revision, scope, content hashes, payload-contract identity and explicit replacement history.
- Block ambiguous or conflicting current selections, modified inputs and expired authority verification. Require a reason and candidate ID for intentional historical use.
- Add optional bounded Kev contradiction classifications with validated answers, independent from deterministic selection.
- Recheck inputs after model review and document upstream discovery and downstream enforcement boundaries.
- Preserve the existing six tool interfaces and shared Kev service.

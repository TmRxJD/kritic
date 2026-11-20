# Recover an unavailable owner's worktree

Kev Tools **0.5.0 or later** provides `worktree_evaluate(action="recover")`.
Use it when the user explicitly requests continuation and the previous agent
cannot cooperate: exhausted tokens, lost session or crash. Normal cooperative
`transfer` remains available. Recovery requires neither old-owner release, a
new source handoff nor an observed source turn boundary. No GPU/model is needed.

## Fresh-session workflow

1. Start the successor in the **existing worktree**, preserving its staged,
   unstaged, deleted and untracked work. Do not create a replacement tree snapshot.
2. Discover `action="schema"`; use `action="status"` with the actual root to find
   the exact editing `actor_id`, `generation`, task and scopes. Do not guess from
   a title, heartbeat, file timestamp or session import. Multiple trees have
   separate assignments; recover each using its own actual successor identity.
3. Call `action="recover"` with `phase="inspect"`, your actual `host`,
   `session_id` and optional `agent_id`, the `worktree`, `source_actor_id`,
   `expected_generation` and explicit `authorization`:

   ```json
   {"kind":"user_request","user_request":"Claude hit its limit; let Codex resume this worktree.","reason":"The source cannot release ownership or prepare a handoff."}
   ```

   Record the actual user's instruction; the quoted example is not authorization.
   Inspection changes no ownership and does not freeze the source.
4. Review the returned `plan` and `plan_sha256`. Preserve them in permanent
   project data such as `.kritic/recovery/<assignment>.json`, ignored by Git.
   The plan binds the inherited task/scopes, current source generation/activity,
   successor identity, HEAD, full staged object identities, dirty/untracked file
   hashes and pending transfers. Reconstruct missing context from current sources
   and task records; preserve uncertainty instead of inventing a successful handoff.
5. Call `action="recover"` with `phase="accept"`, the same actual identity,
   the unchanged `plan`, `plan_sha256`, and `acknowledge_context_gaps=true`.
   A different agent in the same source host/session is not a fresh successor.

Acceptance re-derives the live plan within one ownership transaction. Changed
generation, source activity, index/files, successor assignment or pending transfers
requires a new inspection. It atomically demotes/fences the source, inherits the
exact task/scopes and invalidates pending transfers from that source. No checkout,
merge, reset, file write or cleanup occurs. The old session cannot silently reclaim
writing after it resumes. Other trees and assignments stay intact.

## Setup and blocked agents

Recovery is reachable before a declaration: matching Codex, Claude Code and Hermes
hooks permit both the MCP action and the exact installed coordinator's restricted
native-command bootstrap. The hook validates your identity. Use the native command
tool with `python -I <matching-coordinator.py> --request-json <JSON>` or the
argv/stdin `cli_run` form shown by the installed hook. Get the coordinator's exact
path from that hook or configured companion; do not execute a remembered copy.
Reconnect clients after upgrading the companion to load its new MCP schema.

A successor with another durable assignment must explicitly resolve its own claim
first. Other editing conflicts are not forcibly discarded. An inactive heartbeat
never authorizes eviction. A missing old conversation is not a passing context
review, and recovery never blesses old fixtures or authorizes merges.

## Trust and limits

The user instruction/reason is caller-supplied and audited, **not authenticated**
by this local cooperative coordinator. A plan hash binds bytes and detects drift;
it does not prove a human or agent understood them. No model can authorize recovery.
Hooks cover enrolled clients and supported events. They cannot stop a command
already running, a pending unhooked write, or direct filesystem/database access.
Keep the old task stopped; investigate unexpected ongoing writes before proceeding.
Snapshot admission remains bounded to 256 dirty paths, 20 MB per file and 128 MB
total; excluded/oversized content requires explicit resolution, never deletion.

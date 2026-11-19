# Worktree intent and scope coordination

This opt-in integration gives Codex, Claude Code and Hermes one persistent repository coordination registry. It records each agent's task, change explanation, worktree and owned file/directory scopes. A read-only evaluator compares worktrees against an explicit main ref and produces a plan containing only individually reviewed hunks.

Git commands in the coordinator inspect metadata and changes. It never merges, cherry-picks, resets, checks out, prunes, deletes worktrees or discards dirty files. A scope declaration states intent. It is not proof that implementation is correct or that a merge has passed tests.

## Declare ownership

Use Kev Tools `worktree_evaluate` with `action=schema` for the exact request schemas. Then declare the identity shown by the harness hook, the actual Git worktree root, a task, a change explanation and scopes with purposes.

```json
{
  "action": "declare",
  "request": {
    "host": "codex",
    "session_id": "SESSION_ID_FROM_HOOK",
    "agent_id": "main",
    "worktree": "B:/dev/project-feature",
    "task": "Add a bounded import pipeline",
    "change_summary": "Implement import validation and its regression tests",
    "scopes": [
      {"path": "src/imports/", "purpose": "Import parsing and validation"},
      {"path": "tests/imports.rs", "purpose": "Import regression coverage"}
    ]
  }
}
```

Use `host=claude` or `host=hermes` for those harnesses. Agent IDs must come from actual event/task metadata. Hermes uses its actual task ID when present. A declaration names 1..64 exact files or directory prefixes. Directory prefixes end in `/`. `src/foo/` does not reserve `src/foobar/`. Paths cannot escape their worktree or claim shared `.git` metadata. Existing symlinks/junctions are resolved before structured writes are authorized. Optional access=read_only still declares a review task and read scope, but does not reserve an editing scope or require a separate editing tree. Structured writes are denied when its actual reader identity is available.

When multiple registered agents are in the same Git repository, each caller must declare before its own edits. An undeclared peer cannot edit, but does not freeze a properly declared writer. Editing agents use separate worktrees and nonoverlapping scopes, except for the explicit same-worktree succession protocol below. Repository identity comes from the shared Git common directory, so linked worktrees count together. Reservations and conflict checks use a SQLite transaction. Simultaneous overlapping claims cannot both succeed. Prepare each child's worktree before delegation when possible. Host-managed worktree creation and structured `cli_run` Git worktree creation remain available for setup; forced creation is not a bootstrap exception.

Narrow or reassign conflicting scopes before proceeding. Declarations can be updated, which changes their generation. Released and replaced intent is retained in history. Do not bypass a denial through a different tool or silently modify another agent's declaration.

Use `action=status`, `request={"worktree":"<absolute root>","include_history":true}` to inspect active claims and recent intent history. `action=release` takes `host`, `session_id`, optional `agent_id`, the inspected `expected_generation`, and a reason. Release updates registry metadata and preserves every worktree file. Missing heartbeats mark a claim suspect after 30 minutes. They do not automatically free it or delete work. Session-end events change runtime presence only: editing ownership survives process exit and resume until explicit release or acknowledged transfer. If a session has not reconnected to the new MCP tool, `cli_run` can invoke the exact installed coordinator with `program=python`, `args=["-I","<absolute coordinator.py>"]` and a schema/status/declaration/release/transfer JSON request in `stdin`.

## Resume the same worktree in a fresh session

Use `action=transfer`; request `action=schema` for the exact phase fields. This requires matching updated coordination adapters on both sessions. This protocol transfers registry ownership; it does not launch a model, approve a handoff's meaning, merge branches, or grant operating-system filesystem permissions.

1. The current editing owner finishes its turn. The native Stop event records the observed turn boundary. Produce a covered [handoff report](../../docs/session-continuity.md) with pinned context, current revision, dirty files, selected fixtures, and exactly the declared write scope.
2. The source calls `phase=prepare` with its actual identity, inspected `expected_generation`, and `handoff_report={path,sha256}`. The existing tree is reserved and the source becomes quiescent. No file, index entry, branch, or dirty content is changed.
3. After the host actually starts a fresh session, the source calls `phase=bind` with its transfer ID, inspected transfer generation, and the actual destination host/session/agent identity. A pending desktop card is not proof of startup.
4. The successor reads and independently checks the pinned handoff and evidence. It calls `phase=accept` with its own actual identity, transfer ID/generation, handoff hash, and complete `accepted_user_event_ids`, `accepted_fact_ids`, and `accepted_fixture_ids`. Acknowledging IDs is a delivery contract, not proof of understanding.
5. Acceptance rechecks ownership generation, user-event epoch, exact HEAD/index/dirty content and evidence hashes in one serialized registry transaction. The successor inherits the existing tree, task, change explanation and scopes. Every previous writer remains fenced; its process ending or reopening cannot reclaim the tree. The successor can exit and resume without losing its durable assignment.

Use `phase=status` to reconcile an uncertain result before retrying. The source may `phase=cancel` an unaccepted transfer with a reason; this restores its writer role without modifying files. Changed evidence, new user input, a changed workspace, wrong acknowledgements and concurrent acceptance fail closed. Preserve both sessions and the work; never reset the tree or infer abandonment from a missing heartbeat. These hooks cover supported enrolled events only, so matching adapter versions and exclusive external-process ownership remain prerequisites. A separately authorized host controller is still required for unattended fresh-session launch and recovery.

## Inspect worktrees against main

Refresh the intended remote ref separately and check that it represents the actual integration target. The coordinator does not fetch or guess which branch is authoritative.

```json
{
  "action": "inspect",
  "request": {
    "worktree": "B:/dev/project-main",
    "main_ref": "refs/remotes/origin/main",
    "worktrees": ["B:/dev/project-feature"]
  },
  "use_kev": false
}
```

Omit `worktrees` to inspect the repository's registered Git worktrees. The tool reports unavailable roots instead of recreating or removing them. Optional `paths` selects exact repository-relative files or directory prefixes, with the selection recorded in the report; unselected paths are outside that review. Limits are 32 selected worktrees, 128 changed paths per tree, 128 KB per text artifact, 500 KB of patch evidence and a 30-second inspection budget. Narrow the selected trees or paths when a limit is reached. Oversized requests fail with an explanation rather than silently approving incomplete coverage.

Each worktree report contains the main revision, merge base, branch head, dirty-state indicator, dirty/untracked path inventory, applicable owner actor ID and intent generation, changed paths and exact hunk identifiers. It distinguishes changes already integrated, changes outside declared scope, paths where main also changed and candidates needing intent review. Blob IDs and Git file modes are compared, so permission changes are preserved for review.

Active editing claims take precedence over closed claims at a reused worktree. For ambiguous historical declarations, inspect status/history and pass owner_actor_ids mapping the selected worktree to its inspected actor ID.

Dirty and untracked work is reported and preserved. The evaluator's committed-change scope cannot certify those files for integration. Preserve and review them before requesting inclusion. Existing changes must never be discarded merely to make a worktree appear clean.

## Isolate intended changes

Read each proposed diff against its declared task and scope. Identify unintended changes, dependencies, main-branch changes and evidence gaps. Give every changed hunk an explicit `include`, `exclude` or `defer` decision with a grounded reason. Return those decisions through `action=evaluate`.

```json
{
  "action": "evaluate",
  "request": {
    "snapshot_request": {
      "worktree": "B:/dev/project-main",
      "main_ref": "refs/remotes/origin/main",
      "worktrees": ["B:/dev/project-feature"]
    },
    "main_revision": "MAIN_COMMIT_FROM_INSPECT",
    "reviews": [
      {
        "worktree": "B:/dev/project-feature",
        "head": "FEATURE_COMMIT_FROM_INSPECT",
        "owner_actor_id": "EXACT_OWNER_ACTOR_ID_FROM_INSPECT",
        "owner_generation": 2,
        "hunk_id": "EXACT_HUNK_HASH_FROM_INSPECT",
        "decision": "include",
        "reason": "Implements the assigned import validation boundary"
      }
    ]
  }
}
```

The evaluator reinspects the repository and rechecks registry identities/generations after inspection and review. Main/head changes, stale hunk IDs, a different owner actor, changed ownership generations, missing decisions, scope gaps and unresolved main changes prevent a usable inclusion plan. Binary, large, empty-file and mode-only changes receive fingerprinted file-level review units using the same decision fields. Included units become isolated_files operations pinned to exact Git objects and modes, without copying binary data into model context. Submodule inclusion remains a separate review gap. An empty or incomplete review never passes.

`status=review_plan` contains the isolated patches and review reasons. It is a preparation result, not merge approval. Apply chosen patches to a separate integration worktree, use `git apply --check`, inspect their combined dependencies and run the real repository validators. For isolated_files, verify the integration target is at the recorded main revision and its affected paths are clean before using the returned integration-only restore arguments. Different included worktrees touching the same file are withheld pending combined review. Byte-identical duplicate selections are deduplicated with their source provenance retained. Recheck main and source fingerprints before the authorized merge. Never copy an old worktree snapshot over main. Never delete excluded or deferred work.

Optional `use_kev=true` asks the existing local typed classifier for `aligned`, `unrelated` or `uncertain` labels against task intent. It reviews at most 16 changes per call and explicitly reports any unreviewed count. Metadata, previews and reasons are untrusted data. The classifier cannot approve a merge, prove semantic independence or override ownership and repository checks.

## Install harness adapters

### Recovery when the declaration tool is missing

Tool discovery and agent messaging remain available before declarations. The hook
prints the caller's full actual host/session/agent identity and a ready-to-edit
`python -I <installed coordinator.py> --request-json '<JSON>'` command. Run that
exact coordinator through the native command tool (`exec_command` or `Bash`),
replacing the task/change explanation, actual worktree and scopes. This restricted
bootstrap is accepted even without the Kev Tools MCP tool or Kritic CLI tool.
It accepts only schema/status/declaration/release operations; appended shell commands
and impersonating another actor are rejected. Reconnect MCP afterwards to expose
`worktree_evaluate`. Ask each undeclared peer to declare its own task and scope;
do not guess their assignments or release their claims to unblock yourself.

The test.13 Windows ZIP and Apple Silicon archive include `bin/kev-tools` (`.exe`
on Windows) and its matching `bin/coordination/` directory. Extract the whole
archive into permanent storage. Register a separate stdio MCP server named
`kev-tools` with `command` set to the executable's absolute path and `args=[]` in
each client you use. Keep your existing `kritic` server registered too. With a
source checkout, run `cargo build -p kev-tools --release --locked` and copy
`integrations/coordination/` beside that binary, or set `KEV_COORDINATION_SCRIPT`.

Run `python <extracted>/integrations/coordination/deploy.py --runtime
<extracted>/bin/coordination` to enroll Codex and Claude. Add `--hermes-home
<actual Hermes home> --hermes-kev-exe <absolute executable>` for Hermes; install
PyYAML first when using those options. The installer defaults to `~/.codex` and
`~/.claude`; `--help` lists overrides. This enrollment is optional and modifies
client hook configuration. Reconnect the clients, call `worktree_evaluate` with
`action=schema`, and verify an out-of-scope test edit is rejected in a disposable
test repository before relying on enforcement.

The stdlib Python integration is shared by the existing Python hook/plugin interfaces. Kev Tools provides a Rust MCP bridge. Install the six runtime Python files beside Kev Tools under `coordination/`, or configure the MCP server's `KEV_COORDINATION_SCRIPT` with an absolute `coordinator.py` path. Python 3.11+ and Git are required. Harnesses sharing a repository must use the same permanent `KRITIC_COORDINATION_DIR`.

On this Windows host, the default registry is `B:/AI/coordination/registry.sqlite3`. On WSL with that mounted directory it is `/mnt/b/AI/coordination`; otherwise Unix defaults to `~/.local/share/kritic/coordination`. Check actual filesystem/locking behavior before relying on a shared registry across different hosts or network filesystems. The integration does not synchronize registries over a network.

`deploy.py --runtime <new permanent runtime directory> --hermes-home <actual Hermes home>` installs additive Codex/Claude lifecycle and pre-tool hooks, plus the Hermes plugin. Optional --hermes-kev-exe <absolute Kev Tools executable> adds a missing Kev Tools MCP entry to Hermes. It preserves existing hooks and plugins. On Windows the Hermes plugin is a verified junction to permanent runtime storage. Deployment requires PyYAML only when modifying Hermes config. Existing sessions and running Hermes processes must reconnect/restart to load newly installed hooks/plugins.

### Coverage boundaries

- Codex and Claude adapters observe their supported hook events. Hosted/specialized tools that bypass the hook path remain outside enforcement.
- Codex subagents can share the parent's `session_id` without an `agent_id` in pre-tool events. Captured subagent lifecycle events and distinct declared worktrees let the integration map a write to its owner. That mapping does not cryptographically identify the calling subagent. If there is no unambiguous owner, edits are denied.
- Structured file edits and supported patch headers are checked against scopes, including rename destinations. Unknown structured write formats are denied when coordination applies.
- Arbitrary shell programs and opaque MCP tools cannot be reliably reduced to filesystem write paths. They require a declared owner and explicit worktree cwd, but effect containment remains cooperative. Agents must obey the declaration; use a real filesystem sandbox when hard containment is required.
- Hermes reuses its installed task/path and patch parsers. Registration probes the real installed plugin dispatcher to verify `block` directives are honored. Unsupported dispatcher versions fail setup explicitly. Remote/container task namespaces need a coordinator in that namespace.
- Observations cover enrolled sessions. A session that never loaded the adapters cannot be counted reliably. No inferred process count is presented as proof of every running agent.

Primary harness protocol references are the [Codex hooks documentation](https://developers.openai.com/codex/hooks/) and [Hermes hooks documentation](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/hooks.md).

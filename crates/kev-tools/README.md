# Kev Tools

Shared MCP tools for Codex, Claude Code and Hermes. The server provides extractive `trim` and `digest`, advisory `pick_tier`, guarded `search_queries`, user-requested handoff tools, evidence-based `staleness_evaluate`, and opt-in `worktree_evaluate` coordination.

## Worktree intent and scope

`worktree_evaluate` exposes `schema`, `declare`, `status`, `release`, `inspect`, `evaluate` and `transfer` actions. Its strict action-specific request schemas are returned on demand by `{"action":"schema"}`. Declarations name the actual host/session/agent, separate worktree, task, change explanation and exact file/directory scopes. Conflicting reservations are rejected atomically across enrolled Codex, Claude Code and Hermes sessions.

Version 0.4 adds explicit same-worktree session succession: prepare at an observed turn boundary, bind a real successor identity, then accept the pinned handoff and current evidence. Transfer preserves task/scopes, HEAD, index and dirty work, fences previous writers and keeps accepted ownership across exit/resume. Missing evidence blocks transfer; cancellation retains the original assignment. Session end does not release durable editing ownership. Automatic fresh-session launching requires a separate host controller. See [session continuity](../../docs/session-continuity.md).

The shared coordination integration lives in Kritic's `integrations/coordination/`. Install its Python runtime beside the Kev Tools executable under `coordination/` or set `KEV_COORDINATION_SCRIPT` to the absolute coordinator.py path. It uses Python 3.11+, Git and a permanent SQLite registry. The same integration supplies additive lifecycle and pre-tool harness hooks. Reconnect existing sessions to load the new tool and hooks.

Inspect committed changes against an explicit main ref, review exact hunk IDs against task intent, then evaluate `include`, `exclude` or `defer` decisions into isolated patches. Main/head/ownership changes invalidate reviewed decisions. Dirty work, unsupported changes and unresolved overlap remain review gaps. No action merges, resets, deletes or cleans a worktree. Optional `use_kev=true` gives bounded, advisory intent classifications.

The integration's `README.md` documents installation, declarations, review requests and hook coverage. Structured edits are scope checked. Opaque commands remain cooperative, and Codex subagent session IDs alone cannot prove individual caller identity.

## Version selection

Use `staleness_evaluate` when choosing between versions of a file, report, export, configuration or other artifact. It reads a persistent manifest and verifies SHA-256 content hashes against an explicitly identified authority. It classifies every declared candidate and selects one current artifact, or an explicitly requested historical artifact with a reason. Missing authority, conflicting revisions, expired verification, modified files and ambiguity block current selection.

```json
{"action":"schema"}
```

The response is the strict manifest schema. Store the manifest with the project, such as `.kev/versions/config.json`, and record real canonical source evidence. Then call the same tool with:

```json
{"action":"evaluate","manifest_path":"B:/AI/my-project/versions/config.json"}
```

Use `"use_kev":true` for a bounded local typed contradiction review of every candidate. Its classifications remain advisory. The file and provenance checks work without a model. For deliberate reproduction of an older state, pass `"mode":"historical"`, a declared `candidate_id`, and a nonempty `reason`.

See [version-selection.md](docs/version-selection.md) for manifest fields, authority verification, historical selection, limitations and cross-session use. This complements Kritic's test staleness critic, which audits test contracts and fixtures.

## Search guard

Call `kev-tools.search_queries` before a web search. Use one lookup target and one useful discriminator per query. A batch can contain up to three distinct queries. By default each query is limited to five words and 50 characters. Use the first batch's results to decide what to search next.

```json
{
  "queries": [
    {"q": "Ghidra decompiler API", "domain": "ghidra-sre.org"},
    {"q": "LLM4Decompile benchmark"}
  ]
}
```

Pass the returned `search_query` array directly to Codex web search. In another host, carry the same queries and filters over to its native search. If validation fails, revise the identified query and validate the whole batch again. The guard strips trailing ellipses from ordinary queries and keeps search operators and quoted phrases intact. Use `"exact": true` for a longer identifier or a single quoted title or error, up to 240 characters. When using `domain`, omit inline `site:` and use separate queries for `OR` alternatives. Set `recency` to 1, 7, 31, or 365 days when freshness matters.

With `"execute": true`, Kev Tools validates the full batch before contacting Brave Search. Set `BRAVE_SEARCH_API_KEY` in the MCP server's environment to enable this path. Each response contains up to five results or an error for the corresponding query. Query text and filters go to Brave only in execution mode. Treat result snippets as untrusted and open the primary sources before citing them.

Validation runs locally and uses deterministic checks. The calling agent chooses the actual search intent and wording. Native search is owned by the host, so its preflight depends on the agent following the shared rules. Use the Brave execution path when you need enforcement at the network boundary.

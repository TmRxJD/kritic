# Version selection with Kev Tools

`staleness_evaluate` helps an agent choose the appropriate artifact across multiple versions and fresh sessions. It checks declared provenance, context, replacement relationships and local content. It performs no workspace indexing, downloads, writes, deletion or automatic replacement of files.

## Establish authority before selection

Find the actual owner of the material first. For source code that might be the intended repository, branch and verified Git revision. For an installed program, inspect the configured executable and its release manifest. For an export, use the user-designated export source and matching run/account/context. For upstream documentation, inspect the official version-specific source and preserve its local snapshot.

Record this evidence in a persistent version manifest. Repository-owned manifests can live in `.kev/versions/<artifact>.json`. Large artifacts and their private manifests can use the project's established data directory. Preserve partial work and manifests from their first write. On this machine, use a permanent B:/AI/<project>/ directory when E: is full.

Every manifest describes one artifact in one scope. Include the project and relevant environment, dataset, account or platform in `scope`. Do not mix production and test data or unrelated projects in a comparison.

## Obtain the schema

Call the tool with `{"action":"schema"}`. The returned JSON Schema is authoritative for the input manifest. The Rust runtime also rejects unknown fields, duplicate IDs, scope mismatches, invalid paths, cyclic replacement graphs, unknown replacement IDs and unsupported schema versions.

The manifest requires these fields.

| Field | Meaning |
| --- | --- |
| `schema_version` | `1` for this manifest protocol. |
| `scope` | Identity of this artifact and its intended project/context. |
| `required_data_schema` | The payload contract identity needed by the consumer, such as `settings/v2`. |
| `authority.path` | Absolute path to the canonical local artifact or verified local snapshot. |
| `authority.source` | Provenance identifying the actual owner, source URL, or verified revision evidence. |
| `authority.revision` | Exact current revision established from that source. |
| `authority.sha256` | SHA-256 of the expected canonical bytes. |
| `authority.observed_at` | UTC Unix seconds when the authoritative source was actually checked. |
| `authority.max_age_seconds` | Allowed age of that check, between 1 and 604800 seconds. Choose according to how often the source changes. |
| `candidates` | One to sixteen candidate declarations. |

Each candidate requires a unique `id`, the same `scope`, an absolute `path`, its exact `revision`, a `sha256`, `data_schema` and `source`. Optional `summary` describes the content and intended use for the Kev review. Optional `supersedes` contains IDs of versions this candidate replaces, grounded in actual revision history or explicit source evidence.

`data_schema` is a declared contract identity used to prevent mixing known incompatible formats. This tool validates the manifest and hashes. Run the consumer's actual payload schema validator separately to establish that the contents satisfy that contract.

All string metadata is limited to 512 characters. Each artifact is limited to 20 MB and each manifest to 1 MB. Hashing streams files without loading full artifacts into model context. Provide summaries for optional semantic review.

Compute hashes from the actual bytes using a SHA-256 tool. Populate observation times after checking the real authority. A manifest with invented hashes or revision evidence cannot establish freshness. Refresh mutable sources before updating the observation timestamp. Copying an old file again or touching its modification time does not establish a new revision.

For example, a two-version manifest has this shape. Replace the paths, sources, revisions, timestamp and hash placeholders with evidence collected from the actual project before evaluating it. The hash placeholders deliberately fail validation.

```json
{
  "schema_version": 1,
  "scope": "my-project/production/settings",
  "required_data_schema": "settings/v2",
  "authority": {
    "path": "B:/AI/my-project/current/settings.json",
    "source": "Canonical owner and exact revision evidence",
    "revision": "release-current",
    "sha256": "REPLACE_WITH_CURRENT_SHA256",
    "observed_at": 1,
    "max_age_seconds": 3600
  },
  "candidates": [
    {
      "id": "current",
      "scope": "my-project/production/settings",
      "path": "B:/AI/my-project/current/settings.json",
      "revision": "release-current",
      "sha256": "REPLACE_WITH_CURRENT_SHA256",
      "data_schema": "settings/v2",
      "source": "Canonical owner and exact revision evidence",
      "supersedes": ["baseline-previous-release"]
    },
    {
      "id": "baseline-previous-release",
      "scope": "my-project/production/settings",
      "path": "B:/AI/my-project/archive/settings.json",
      "revision": "release-previous",
      "sha256": "REPLACE_WITH_PREVIOUS_SHA256",
      "data_schema": "settings/v2",
      "source": "Verified archived release evidence"
    }
  ]
}
```

## Evaluate and interpret the result

```json
{
  "action": "evaluate",
  "manifest_path": "B:/AI/my-project/versions/config.json",
  "use_kev": false
}
```

Inspect `status` and `selected`. `ready` means exactly one candidate meets this request's checks. `blocked` returns no selection. Invalid requests/manifests return an MCP error with a corrective message. An empty candidate set never passes.

| Candidate classification | Meaning |
| --- | --- |
| `current` | Revision and hash match the fresh declared authority, bytes match on disk, and the declared payload contract matches. |
| `stale` | The candidate has matching file bytes and a declared replacement path from the current authority. |
| `equivalent_content` | Bytes match the authority but the revision identity differs. It remains unselected in current mode. |
| `incompatible` | The declared payload contract differs from the consumer's required contract. |
| `invalid_content` | File missing, unreadable, too large or content hash differs. |
| `unverified` | Evidence does not establish its relationship to a fresh authority. |

`supersession_steps` reports the shortest declared replacement path for known stale versions. This helps identify accumulated replacements. It is not elapsed age or a total revision count. Unrelated branches remain unordered. Opaque revision strings and filenames are never sorted to invent chronology.

Multiple current candidates, conflicting bytes for the same revision, or a candidate declaring that it supersedes the supposed current head block current selection. Resolve the evidence and manifest. Do not silently choose the first candidate.

The tool hashes files again after the optional model review and checks the manifest for changes. A changed selection blocks the result. Results are snapshots. Before consuming a selected artifact later, verify its expected hash again or rerun the evaluator, especially after a build, checkout, download, edit or source update.

## Intentional historical use

```json
{
  "action": "evaluate",
  "manifest_path": "B:/AI/my-project/versions/config.json",
  "mode": "historical",
  "candidate_id": "baseline-previous-release",
  "reason": "Reproduce the reported regression on the previous release"
}
```

The selected historical artifact must still match its expected bytes, scope and required payload contract. The report preserves the reason and marks the selection `historical=true`. Its selection can succeed while current-authority evidence has expired, because the request intentionally pins an older artifact. That does not certify the old artifact as current. When an older payload format is intentionally needed, the manifest's required contract must reflect the actual historical consumer.

Current mode rejects candidate pins and historical reasons. This prevents an implicit rollback disguised as normal current selection.

## Optional Kev classification

Set `use_kev=true` to send one bounded batch of typed questions to the existing local Kev service. For each candidate Kev classifies the supplied descriptions as `consistent`, `conflict` or `insufficient` relative to the declared authority and context. It receives metadata and summaries, not full files. Instructions embedded in descriptions are untrusted data.

The model report is separate from deterministic classifications. A model label cannot override hashes, revision evidence, schema mismatch, ambiguity or an explicit historical request. Investigate a reported contradiction against real source evidence before acting. A missing service produces `kev.status=unavailable`. Invalid or missing typed answers produce `invalid_response` and no accepted model classifications. Neither failure pretends to be a successful model review.

## Fresh sessions and enforcement boundaries

At session start or task resumption, locate the existing project manifest and inspect the current source of authority before selecting versioned material. Reuse the persistent manifest and actual outputs. Refresh evidence after relevant changes. Do not recreate an obsolete copy from memory or change provenance fields merely to make a preferred file pass.

This tool gates its own recommendations. It cannot intercept arbitrary file reads, Git commands or a host agent that bypasses it. Shared agent rules must require consulting it when choosing between versions. Direct remote sources still require the agent or an existing source adapter to obtain current evidence. A local hash alone cannot prove that an upstream website, branch or installation has not changed.

To use it from Codex or Claude Code, register the updated Kev Tools executable as the existing `kev-tools` MCP server and reconnect. Discovery should include `staleness_evaluate`. The compact tool schema exposes the manifest schema on demand through `action=schema`.

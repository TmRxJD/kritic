# Technical-debt review — optional source feature

`technical_debt_critic` and `kritic debt` inventory dependency evidence and declared
maintenance work. They need no GPU, model, package-manager install or script execution.
This is advisory prioritization: **there is no approval verdict, automatic upgrade or
proof that the project is free of debt**. The published `v0.5.0-test.3` binary predates it.
Use a current source build; release/CI readiness must be checked separately.

## Start offline

Enable the capability in the server's config, then restart the MCP server/client:

```toml
[capabilities]
debt = true
```

`debt=false` is the built-in and minimal-profile default. `KRITIC_CAP_DEBT=true` is the
environment equivalent. The same capability gate applies to explicit CLI calls.

```sh
kritic debt --base-dir .
# Optional, explicitly selected comment corpus:
kritic debt --base-dir . --source-paths src/service.rs scripts/build.py
```

MCP clients, including Codex and Cursor, call `technical_debt_critic` with:

```json
{"base_dir":".","online":false,"refresh":false,"source_paths":[]}
```

Offline review makes no HTTP requests. It can show previously cached registry evidence
with its date/status; absent evidence stays unknown. A package requirement such as `^2`
is not a resolved version. Lockfile versions are evidence of the declared resolution,
not proof of what a running deployment installed.

## Optional public registry evidence

```sh
kritic debt --base-dir . --online
kritic debt --base-dir . --online --refresh
```

`online=true` / `--online` explicitly permits sending public package identifiers and
relevant versions to public HTTPS metadata services. Review that disclosure before opting
in. No source contents, credentials, environment-variable values or project scripts are
sent or executed. Private, Git, path, workspace and alternate-registry dependencies stay
visible in the inventory and skip public lookup. A private npm project does not make its
public third-party dependencies private. `.npmrc` registry overrides are respected without
printing their credentials.

Cargo, npm and PyPI use their primary registries. Go, Maven, NuGet and RubyGems use public
deps.dev evidence for explicitly inventoried packages. The report records source URLs,
observation dates, lookup status and errors; a default release is an observed registry
target, not a universal recommendation. Inspect the actual response and upstream notes.
Advisory identifiers are triage leads, not an exploitability assessment or replacement for
`security_critic` and the project's security process.

Evidence is cached for 24 hours. `--refresh` requires `--online`; it requests new evidence
and never updates a dependency. Failed lookups remain visible. Stale cached evidence is
labelled and needs refreshing before a maintenance decision. Registry lookup has six
concurrent unique-package requests, a ten-second request timeout and a 2 MiB response
limit. The entire online batch has a 60-second budget. Remaining lookups return explicit
`budget_exhausted` errors; a later call can reuse evidence already cached. One call does not
guarantee metadata for every package. Cache entries are capped at 256 KiB, with 1,024 entries / 64 MiB total; a full cache
requires explicit retention work rather than silent deletion. No forced dependency
updates, credentials, cloud LLM calls or inferred environment-variable requirements occur.

## What is inventoried

| Ecosystem | Automatic manifest/lock coverage |
| --- | --- |
| Cargo | `Cargo.toml` dependency sections, package renames and `Cargo.lock` resolutions |
| npm | `package.json`, npm aliases and `package-lock.json`; v1 nested graphs have explicit gaps |
| pnpm | v9 importers and packages, workspace-specific resolved versions and peer suffixes |
| Python | `pyproject.toml`, `requirements*.txt`, `uv.lock`, `poetry.lock`; explicit unsupported/dynamic records remain gaps |
| Go, Maven, NuGet, RubyGems | Explicit `.kritic/debt.toml` package declarations; detected native formats currently produce coverage gaps |

Older pnpm formats, Yarn locks and other unsupported records are not silently treated as
covered. Direct and locked transitive entries retain their manifest provenance. Ambiguous
versions remain unresolved. Transitive upgrades require finding the owning direct dependency;
do not force a lockfile edit merely because a newer transitive version exists.

The defined corpus is Git-tracked dependency manifests/configs plus standard untracked root
manifests. Without Git, a bounded manifest-only walk is used. Dependency/generated/cache
folders and symlink escapes are excluded or rejected; source code is not indexed. Limits:
200 manifests/configs, 2 MiB each, 16 MiB total and 1,000 dependency entries. Budget errors
and malformed/unsupported formats are reported, never silently truncated into a clean result.

Use [`examples/debt.toml`](../examples/debt.toml) as a commented template. Version 1 supports
explicit `[[packages]]` with `ecosystem`, `name`, optional exact `current`, optional
`requirement`, `scope` (`direct` or `transitive`) and `public_registry`. Ecosystems are
`cargo`, `npm`, `pypi`, `go`, `maven`, `nuget`, `rubygems`. Never put a version range in
`current`, invent a resolved version or mark a private package public to obtain a lookup.

## Urgency and estimated effort

Urgency and effort are separate. A newer release alone is not an emergency. Reported
advisories or current-release deprecation warrant security/migration triage. An expired
project-declared maintenance deadline raises urgency to at least high; the declaration
still requires independent validation.

| Effort | Meaning to investigate |
| --- | --- |
| 1 | Small version-bump candidate; successful upgrade has not been demonstrated |
| 2 | Dependency/lockfile and compatibility review |
| 3 | Configuration, runtime or environment migration plan |
| 4 | Potential breaking API changes requiring investigation |
| 5 | Explicitly declared broad refactor or migration |

Ratings are estimates, with uncertainty ranges and validation steps. Even a patch can break
application behavior; a major version can sometimes be simple. Qualified/prerelease versions
or unsupported ordering remain unordered. A resolved version ahead of the observed default
does not trigger a downgrade recommendation. A deprecated target requires choosing a supported
alternative before estimating work.

Optional version-bound `[[migrations]]` notes name ecosystem/package, exact `from`/`to`,
effort 1–5, a concrete summary and HTTPS `source_url`. Only a matching observed version pair
uses the note. `required_env_vars` contains variable **names only**, never secrets or values.
Notes are project-declared evidence: their linked contents are not fetched or authenticated.

Optional `[[maintenance]]` notes require unique `id`, concrete `summary`, `urgency`
(`low`, `medium`, `high`, `critical`) and HTTPS `source_url`; `effort` 1–5 and
`due_date = "YYYY-MM-DD"` are optional. These are explicit maintenance claims, not findings
that Kritic independently proved.

Explicit `source_paths` selects at most 50 regular in-repository files, 2 MiB each / 16 MiB
total, with at most 1,000 markers. Only leading-comment `TODO`, `FIXME`, `HACK` and
`DEPRECATED` markers become review leads. They are not confirmed bugs, all technical debt,
or evidence that adjacent strings/comments describe real work.

## Act on evidence

Inspect coverage gaps and the saved full report before prioritizing. MCP responses are bounded
and report returned/total counts plus the saved-result reference; a short response is not a
complete inventory. Preserve private reports deliberately.

Choose one coherent maintenance purpose, review upstream migration/runtime/peer requirements,
then use a separate upgrade branch. Update through the repository's normal package workflow,
inspect its manifest/lockfile diff and run meaningful build/typecheck/tests plus affected user
flows. Verify required variable names from actual migration evidence; supply values through
the project's existing secret/config process. Kritic never applies that upgrade or treats an
estimated effort rating as a successful compatibility test.

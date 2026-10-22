# Work contracts and enforcement

For a missing or invalid project manifest, agents should follow the report's structured setup
steps (matching current source) and [agent setup/recovery](agent-setup.md). Identify the exact
repository, derive checks from its real rules, and rerun; do not pass the OS error to the user
as though the model-free critic itself were offline. No default passing contract is invented.

The model-free `contract_critic` reads `.kritic/contract.toml` (version 1). It separates declared
acceptance checks from enforcement wiring. `approval` refers only to the required declared
assertions. It never means all user instructions are fulfilled or deployment is authorized.

Start from [examples/contract.toml](../examples/contract.toml). Each requirement has a unique ID,
a description, `required` (default true), optional `require_enforcement`, checks and enforcement
bindings. Empty contracts, duplicate IDs and missing descriptions are rejected. A requirement
with no checks is unverified. All required criteria must be verified to return true; any required
violation returns false; missing/unrun/stale evidence returns null. An all-optional contract never
returns vacuous approval. `coverage` is required declared criteria verified / total required criteria.

Checks:

- `file_exists`: a bounded readable file exists. This proves presence, not behavior or quality.
- `content`: path + regex pattern + `present` (default true). This proves only the textual assertion.
  Broad matches do not establish behavioral acceptance; agents must inspect assertion quality.
- `command`: a fresh shell command's exit code/output. Runs only with explicit `run_commands=true`.
  Review commands first; they execute with your user's permissions. Exit 0 is not proof meaningful
  tests ran. Check test discovery, counts and whether assertions cover the described behavior.
- `report`: JSON path + JSON pointer + expected value + nonempty map of input paths to SHA-256
  hashes + max_age_s (default 3600). The report must be fresh and hashes must match current inputs.
  Reports are supplied evidence, not signed attestations. An edited report can lie; prefer fresh
  independently run commands and inspect the report's producing tool.

Enforcement bindings specify path and pattern (for example the actual validation command in CI).
`configured` means declared text is present, `missing` means an assertion failed, `unverified`
means missing/unreadable/no binding. These are **static wiring checks**, not proof a hook executed,
CI succeeded, branch protection is enabled or the agent obeyed an approval boundary.
`require_enforcement=true` makes missing wiring violate the requirement; unknown wiring prevents approval.

AGENTS.md, `sources=[...]` documents and the tool's task text are returned for the calling agent
to interpret. They are never silently converted into satisfied requirements. Agents must compare
all relevant instructions against the manifest and explicitly identify unmapped clauses and weak
checks. No regex engine can automatically understand arbitrary prose contracts.

Files are bounded to 1 MiB and must resolve inside base_dir, including symlinks. Git internals are
excluded. Requirement/check/source counts are bounded. Run output includes check time, run ID,
contract hash, evidence hashes and the full report path. Input mismatch makes report evidence stale.

CLI: `kritic contract --base-dir . [--path path] [--task text] [--run-commands]`.
Exit 0/1/2 = verified/violated/unverified. MCP: `contract_critic(base_dir=..., run_commands=false)`.
No model is needed. Your agent may assess prose/coverage but cannot manufacture executed evidence.

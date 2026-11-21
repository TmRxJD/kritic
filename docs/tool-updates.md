# Routine tool updates

The debt critic inventories repository dependency manifests and gives advisory
maintenance findings. It does not automatically update installed applications or
MCP servers. The **operational update gate** handles that separate responsibility.
Its checks run before tool use, rather than as mid-edit implementation critiques.

## Policy and cadence

Use a version-1 `~/.kritic/updates.toml`; keep it separate from the main critic
configuration to avoid old-binary unknown-key failures. An absent policy is not
enrollment. A configured policy defaults to weekly checks (604800 seconds).
`policy.interval_seconds=86400` selects daily checks. Individual tools may override
the interval, with a five-minute lower bound. Examples use explicit paths; replace
them with observed installations, not copied assumptions.

`policy.client_configs` names a bounded corpus of actual Codex/Claude/Hermes MCP
registrations. Discovery recognizes installed Node package manifests and Python
console-script distributions. Explicit tool profiles override discovery for a
server. Transient `npx`, custom launchers, private/unrecognized packages and native
development builds need a grounded provider or an explicit exclusion with reason.
Inventory reports remote host-managed services separately: this integration cannot
download a provider's cloud service or change the host app's bundled tools.

Per-tool metadata is shared across agents in SQLite, keyed by exact policy/source/
binding identity. Successful lookups advance `checked_at`; offline/invalid responses
do not. Failed attempts have a separate retry backoff so repeated calls do not
hammer the network. Changed sources invalidate cached evidence. Future timestamps
are not treated as fresh. A transaction/lease allows one check/download per tool.

Session-start inspection is local-only. The first use of an expired managed tool
starts one hidden metadata worker and blocks that use until it finishes. Subsequent
fresh-cache calls make no update HTTP request. Version probes still verify the
configured local installation; their result is not inferred from a filename.
Local version observations are cached too: fresh calls do not repeatedly launch
Blender/Node probes. Package metadata hashes and executable identity/stat changes
invalidate that cache, and interval expiry forces re-probing. Timestamps only
detect changes; they never decide which version is newest or prove adversarial
file integrity. Checks do not run periodically for idle tools; `check` can also be scheduled by an
explicitly configured administrator, but no scheduler is silently installed.

## Available, downloaded, installed, active

These are distinct states. Stable numeric versions are compared without automatic
downgrades or prerelease promotion. Supported official sources are npm, PyPI,
GitHub release metadata and Blender's official mirror tags. Blender downloads use
the official release archive plus its SHA256 file. npm requires registry SHA512;
PyPI and GitHub assets require their upstream SHA256. Redirects are restricted to
declared official download hosts. Credentials, prompts and project source are not
sent; public tool/package identities are sent to their metadata providers.

A policy-compatible update is downloaded by a bounded hidden worker to permanent,
versioned staging. Integrity failures leave it unverified and tool use blocked.
The worker does not extract archives, run installers or install scripts, rewrite
registrations, reopen projects, kill processes or declare a staged package active.
Complete the supported tool manager's installation, run the relevant health/API/
add-on checks, then update the canonical profile and registration. Existing files
and old versions remain available for rollback.

The gate compares supported npm/Python registrations with their canonical package
and runtime. A freshly started session cannot use an observably old registration
merely because a newer package exists elsewhere. Direct executable invocations
must also use the canonical path; stale aliases/paths are rejected. Registration
changes during a session produce `reconnect_required`. A client must actually
reload its MCP server/schema. This integration has no universal host reconnect API
and does not attest the identity of an already-loaded server from a changed file.
Application adapters must verify the live application's version when connecting.

## Migration conflicts and intentional older versions

Semver alone does not prove an upgrade safe. The selected compatibility boundary
is a conservative download policy, not semantic migration approval. Cross-major
(and pre-1.0 cross-minor) changes, changed peer requirements, unmet Node/Python
constraints and configured upstream migration-note warnings require review.
Optional versioned changelog checks inspect only unapplied releases. Unknown
constraint syntax remains unverified rather than guessed compatible.

The agent should resolve routine prerequisites and finish a compatible update
within its authorized task. Preserve existing observations, fixtures, model state,
active sessions and user files. A known migration conflict can be recorded as:

```toml
[tool.pin]
version = "6.7.0"
latest = "6.10.1"
reason = "Evidence-format and runtime migration needs verification; preserve old observations."
evidence_url = "https://github.com/morluto/rea/blob/rea-agents-6.10.1/CHANGELOG.md"
```

This is an explicit current/latest-bound exception, not an eternal pin. It allows
the existing tool with a notice once per session/change and expires when the
observed latest version changes. A reason/URL is an administrator/agent declaration,
not independently authenticated proof. Verify its evidence. Do not disable the
gate, change versions/hashes to pass, or invent migration conflicts to avoid work.
Hermes does not inject arbitrary context from a pre-tool hook. Its adapter delivers
this notice once as a blocked tool result; retrying the unchanged call then permits
the recorded exception. Ordinary fresh/current calls remain silent.

## Agent recovery and coverage

Discovery, exact coordination/recovery operations and the exact installed updater
bootstrap stay available before ordinary tool use. Repair commands include the
actual policy path. Managed MCP methods cannot bypass checking by calling
themselves `read_file`, `send_message` or another control-plane leaf name.

Use `inventory` to inspect all managed tools, exclusions and provider gaps. Enroll
new local sources before relying on their freshness. Exclusions must state why a
tool is intentionally a development snapshot or outside local update authority;
they do not certify its version. Opaque code-mode batches, nested shell scripts,
dynamic aliases, remote containers and unsupported host hook events are not
universally interceptable. Consult this gate for the actual inner tools before
opaque execution; never claim blanket enforcement on an uncovered route.

## Observed examples, not blanket claims

An npm package's own manifest is the package-version authority. A real BlenderMCP
2.3.1 startup reported `serverInfo.version=1.30.0`, the SDK version; treating that
as the package version would be wrong. Likewise a stale add-on setup record saying
Blender 5.2.2 did not establish the application's version: the actual executable
reported 5.3.0. Live connections and add-on protocol compatibility need their own
evidence. No critic/model score can substitute for those checks.

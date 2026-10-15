# Configuration

Read `$KRITIC_CONFIG`, otherwise `$KRITIC_HOME/kritic.toml`, otherwise `~/.kritic/kritic.toml`.
Precedence is environment variable → TOML → built-in default. Use an absolute KRITIC_HOME
for models, reports, logs, keys and caches. All accepted keys and environment aliases are in
[the annotated config](../examples/kritic.toml); tests keep that list synchronized.

## Deterministic baseline

```toml
[review]
backend = "off"
[vision]
backend = "off"
[capabilities]
web = true
ux = true
security = true
quality = true
desktop = true
resources = true
contracts = true
repository = true
history = false
debt = false
plugins = true
context = false
domain_maps = false
[tools]
disabled = []
```

No GPU, inference downloads or reviewer API calls are needed for this profile. Optional tool
dependencies can still be missing; status/doctor expose configuration and availability, while
each result reports what actually ran. Capability toggles hide tools from MCP discovery and
execution. Explicit `tools.disabled` exclusions take precedence. Restart the server/client.

`capabilities.repository` (default true in current source) exposes `repository_critic` and enables
explicit repository CLI checks. The minimal profile sets it false. Set it true deliberately for
[repository-owned policies and hooks](repository-policy.md); installing Kritic does not install a
Git hook. These repository features require a current source build, not the published
`v0.5.0-test.3` binary. Repository rules live in `.kritic/repository.toml`, separately from this
user/server configuration; unknown repository-policy keys fail rather than being ignored.

`capabilities.history=false` is the built-in and minimal-profile default. Opt in with `history=true`
or `KRITIC_CAP_HISTORY=true` for `review_history` / `kritic history`; no model or embedding is
needed. It retains selected reports privately in `runs_dir/history`, hashes explicitly supplied
inputs/context, and exports a separate low-trust jcode-format file. It never reuses a historical
pass as current verification. [Commands, privacy, bounds and limitations](memory-adapter.md).

`capabilities.debt=false` is the built-in and minimal-profile default. Set `debt=true` or
`KRITIC_CAP_DEBT=true` for source-only `technical_debt_critic` / `kritic debt`. It needs no
model and defaults to no HTTP. Registry lookups require explicit per-call `online=true` /
`--online`; `refresh` additionally requires that online opt-in. Repository declarations live
in `.kritic/debt.toml`. [Coverage, privacy, cache, ratings and upgrade workflow](technical-debt.md).

`review.backend`: off, local, cloud, or explicitly selected auto (discover local installation
only; never fall back to cloud). `kev.backend` is a legacy alias; conflicting values disable
inference and produce a config diagnostic. Fresh installs default to off. Invalid backend and
capability values fail closed. Inference refuses configuration errors rather than silently
sending data through another backend.

`vision.backend` is independently off/local/cloud. Local uses Ollama. Cloud uses the selected
cloud transport and `vision.cloud_model` (defaults to cloud.model). The model must accept images;
JEV is a text decision backend, not a cloud vision backend. Cloud vision scores are advisory.

## Credentials and network

Explicit cloud configuration selects provider, full API endpoint, model and `key_env`/`key_file`.
Only the named environment variable or key file is read; keys are never stored in TOML or
printed in Debug output. Default saved credentials are bound to provider and endpoint origin;
a change requires a new save. Explicit key_file paths are an intentional credential selection. `kritic key` prompts for the configured credential. Keep API keys
out of contract commands, test artifacts and source-control commits.

HTTPS is required except loopback HTTP mocks/services. Endpoint userinfo, query strings and
fragments are rejected; authenticated redirects are disabled. Requests have a timeout, a
1 MiB input limit, a 2 MiB response limit and an output-token cap. There are no automatic retries
or model/provider fallbacks. API failures/invalid output remain unscored. Providers using
nonstandard authentication or query parameters require an appropriate compatible gateway.

`status` makes no paid call and does not start inference unless you explicitly request
`start=true` for a selected local backend. `doctor --online` is an explicit reviewer probe.
Local models are loaded on demand/background warmup only after opting into local review.
Scanner feeds/rules, installers and tested websites can use networks independently of inference.

Cloud provider examples: [cloud.md](cloud.md). Contract execution is a separate per-call opt-in:
`run_commands=true` / CLI `--run-commands`, after reviewing the manifest's commands.

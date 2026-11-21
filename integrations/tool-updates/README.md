# Operational tool-update gate

This integration is separate from the advisory, end-of-task debt critic. It checks
official release metadata before using an enrolled tool when its successful check
has expired. **Default: seven days (604800 seconds)**. Use 86400 for daily checks.
No GPU/model is needed. It does not perform an internet lookup on every call.

Use a dedicated `~/.kritic/updates.toml` so older Kritic builds do not reject new
keys in their critic config. See [the full setup/contract](../../docs/tool-updates.md)
and [policy example](../../examples/tool-updates.toml). Keep metadata, verified
downloads, partial results, logs and backups in permanent storage.

```
python -I /absolute/runtime/updater.py --config /absolute/updates.toml schema
python -I /absolute/runtime/updater.py --config /absolute/updates.toml inventory
python -I /absolute/runtime/updater.py --config /absolute/updates.toml check
python -I /absolute/runtime/updater.py --config /absolute/updates.toml status
```

Install matching adapters additively:

```
python -I integrations/tool-updates/deploy.py --runtime /absolute/versioned/runtime --config /absolute/updates.toml --codex-home /absolute/.codex --claude-home /absolute/.claude --hermes-home /absolute/hermes
```

Run it from a trusted source checkout. Existing coordination/recovery, message,
prompt and unrelated hooks stay intact. Repeating the same enrollment is
idempotent. Upgrade a known old update integration with explicit
`--replace-runtime /absolute/old-update-runtime`; arbitrary `updater.py` hooks
are never removed by substring. Private backups and before-image checks protect
client settings. Do not replace immutable runtimes or stop shared services.

Due metadata checks and compatible downloads run in owned hidden workers. Use
stays blocked while required checks/downloads/activation are pending. The worker
verifies upstream SHA256/SHA512 integrity and stages side-by-side; it never runs
package install scripts, changes project files or replaces running processes.
A staged download is **not** an installed or active update. The agent must finish
the tool-specific compatible installation and verify its canonical binding; MCP
clients may require reconnection. Never erase old work to make migration succeed.

Hooks cover supported native events. The optional Hermes adapter verifies that
its real dispatcher honors block directives. Host-managed remote services, opaque
nested execution, custom launchers and tool identities without an authoritative
source remain explicit coverage gaps, not silently certified latest installations.

# Security policy — test release

This release is experimental. Use it on projects you can test, inspect every requested shell
command, and validate findings independently. Kritic tools run with the user's privileges.

Report credential exposure, arbitrary execution beyond the requested contract, path escapes or
unsafe inference routing privately using the repository's GitHub private vulnerability reporting
if available. If unavailable, contact the maintainer through their GitHub profile without posting
secrets or exploit details publicly. Do not assume a private reporting feature is enabled.

Default inference is off. Direct cloud review explicitly sends selected code/screenshot data to
your configured provider. Provider privacy/retention/billing policies apply. Keys are loaded only
from the configured environment variable or key file; don't put secrets in TOML, prompts, contract
commands or reports. Local reports can contain source/output: review before sharing.

Do not rely on scanners alone for authorization/business-logic security. Missing scanner coverage
must be reported. This project does not provide a sandbox for arbitrary project command execution.

See the [test-release security audit](docs/security-audit.md) for fixes, reviewed exceptions and
the disclosed Linux-only dependency maintenance notice. Browser origins cannot subscribe to
the local progress stream; native local tools can. Remote app-event transport is an explicit opt-in.

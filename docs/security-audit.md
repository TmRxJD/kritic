# Test-release security audit

Audit date: 2026-10-10. Scanner approval concerns selected rules and scanned files,
not proof of application security. No model was used for this review.

## Repairs

- Semgrep 1.178.0 partially parsed the installer's compact Bash `case` statement, although Bash
  accepted it. The statement is expanded into multiline syntax. Scanner completion errors now
  retain bounded path/message diagnostics instead of hiding the cause behind a generic error.
- Benchmark test commands formerly ran with `shell: true`. They now dispatch only the two known
  literal commands with argument arrays and no shell. Interpreter probes use literal programs.
- Benchmark fixture requests now reject encoded traversal, malformed URLs, alternate streams
  and paths/symlinks escaping the fixture roots. Tests preserve root priority and reject sibling reads.
- Calibration tests formerly extracted functions with AST compilation and `exec`.
  They now import a pure policy module, preserving the fitting-versus-held-out regressions.
- The loopback progress stream accepted browser WebSocket handshakes. It now rejects every
  Origin-bearing handshake, including opaque `null` origins. A real network regression checks
  rejection and native-client acceptance, preventing unrelated pages reading local metadata.
- App-event loopback validation parses the complete URL instead of inspecting string prefixes.
  Malformed IPv6 authorities and ports are rejected, with regression coverage.
- Legacy UX ignore patterns ran a backtracking regex against page-controlled labels. They now use
  [RE2JS](https://github.com/le0pard/re2js), capped at 64 patterns of 1024 characters each.
  Unsupported backreferences/lookaround fail explicitly. Tests cover a long nonmatching label and
  nested quantifiers. Native Rust regex handling is unchanged.

## Reviewed exceptions

Exceptions name only the rule at the exact code site; no rule is disabled globally and no source
or test directory is excluded to remove these findings.

| Finding | Context and disposition |
|---|---|
| Semgrep child-process in `bench/mcp.mjs` | The operator explicitly chooses the Kritic executable; model/page strings do not choose it. Argument array, `shell: false`. Local tool execution is the benchmark's intended capability. |
| Semgrep plaintext WebSocket in network regression | Test listener binds to loopback and verifies browser-origin rejection. No remote transport. |
| Semgrep WebSocket text in Python port-owner helper | Documentation describes a loopback port, not a connection operation. Its wording was clarified; no code suppression is needed. |
| Bandit B101 in Python tests | Assertions express test expectations, not production authorization/input checks. Only test assertion sites are annotated. |
| Bandit B404/B603 in desktop/Unity/Hermes adapters | Tools intentionally launch the caller-selected local application or installed helper using argument arrays without a shell. They are not a sandbox for untrusted programs. Fixed Python subprocess fixtures are intentional too. |
| Bandit B602 in legacy resource command dispatch (HIGH before exception) | An explicitly caller-supplied string command intentionally invokes the shell, like artifact validation commands. It is never assembled from page/model output by this harness. Callers must inspect commands; argv arrays avoid shell interpretation. This execution capability is retained and disclosed, not described as safe for untrusted commands. |
| Bandit B110/B112 in UI enumeration/cleanup | A control may vanish during UI Automation or cleanup. Handlers preserve bounded best-effort enumeration/cleanup; their absence is not proof that an action succeeded. |

Annotations remain reviewable in source. Changed trust boundaries require reevaluation.
CI blocks Semgrep findings and parser warnings and runs Bandit without a severity floor;
raw reports are uploaded even on failure. Registry rules can change independently of the pinned scanner.

The strict local run scanned 142 files using 500 Semgrep rules and analyzed 3254 Python lines with
Bandit. Semgrep's default ignore policy omitted 53 entries; this is not an all-file coverage claim.
Bandit's annotation-ignored diagnostic retains 175 raw findings for the reviewed exceptions,
including the intentional HIGH shell-command capability. Default Bandit stderr includes seven
`nosec encountered ... no failed test` notices at B603 sites also confirmed in that diagnostic;
these are not parser errors or unknown-rule warnings. CI verifies nonempty coverage from the JSON.

## Dependency maintenance notice

OSV reports [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html) for `paste 1.0.15`:
an informational unmaintained notice, not a scored vulnerability or an available security patch.
The locked Linux-only chain is `netstat2 0.11.2` → netlink packet crates →
`netlink-packet-utils 0.5.2` → `paste`; it is absent from the active Windows target tree.
[netstat2's manifest](https://docs.rs/crate/netstat2/0.11.2/source/Cargo.toml.orig) still requires the
older packet stack. The newer utils release uses a replacement macro crate but is incompatible
with those constraints. Removing the notice requires upstream migration or replacing the Linux
socket enumeration backend. It is disclosed, not suppressed or labeled a vulnerability;
follow-up is tracked in [issue #3](https://github.com/TmRxJD/kritic/issues/3).

## Limits

Live cloud accounts, local models/GPU, desktop GUI and non-Windows runtime behavior were not tested.
Canvas-only effects remain unverified in [issue #1](https://github.com/TmRxJD/kritic/issues/1).
A successful scan is not a promise of zero bugs.

The [Python annotation inventory](security-exceptions.md) lists exact audited sites. An additional
Bandit run with annotations ignored preserves the raw exception evidence; a zero default finding
count does not mean these execution capabilities disappeared.

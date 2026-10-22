# Test staleness and canonical ownership

Test.5 adds the model-free `staleness_critic` and `kritic staleness` CLI. This is a review aid, not a proof that every test is correct/current.
No GPU, model, network, test runner or arbitrary generator is used. MCP is opt-in:

```toml
[capabilities]
staleness = true
invariants = true
```

Reconnect MCP after changing configuration. The standalone CLI needs no capability toggle:

```sh
kritic staleness --schema
kritic staleness check --base-dir /path/to/repo
kritic staleness invariants --base-dir /path/to/repo
kritic staleness sync --base-dir /path/to/repo          # dry run
kritic staleness sync --base-dir /path/to/repo --write  # only declared JSON fixtures
```

MCP: `staleness_critic(base_dir=repo, manifest_path=".kritic/test-contracts.toml")` is read-only.
Missing/invalid configuration returns setup instructions. Match docs/binary versions; old binaries
do not implement this command. Ordinary status stays compact; `status(base_dir)` includes setup.

## Define the source of truth

One `.kritic/test-contracts.toml` owns the graph. `kritic staleness --schema` is the authoritative
machine-readable schema; the repository's manifest is a working, deliberately narrow example.

- `version=1`, `test_roots` and `test_pattern` explicitly bound discovery. The regex matches
  repository-relative forward-slash paths. Include all runner inputs you intend to claim,
  including embedded tests; Kritic does not infer every framework's discovery rules.
- `[[authorities]]`: unique `id`, unique logical `domain`, unique `path`, `owner`, `purpose`.
  Use independently authored requirements/acceptance vectors and canonical schemas/APIs. Add
  relevant lockfiles/configuration as inputs. Two files cannot own the same declared domain.
- `[[tests]]`: `path`, `purpose`, `validation_command`, `reviewed_test_sha256`, and
  `[tests.reviewed_inputs]` mapping authority IDs to exact reviewed lowercase SHA-256 values.
  Hashes are review records, not timestamps or proof a reviewer/test runner ran.
- `[[generated]]`: `path`, `authority`, JSON `pointer` (empty for root), `purpose`, `consumers`.
  Each consumer must be a mapped test bound to that authority. Output is pretty JSON plus a
  newline, exactly the selected canonical value. Keep fixtures outside `test_pattern`.
- Optional `[[exclusions]]` need a test `path` and `reason`. They remain unverified findings;
  they cannot make an all-tests-current claim green. Obsolete exclusions are reported.

The same graph drives fixture projection, read-only inventory and work-contract enforcement.
Do not maintain a second list of expectations or copy an implementation's results into the
contract. A generated oracle from the system under test can reproduce the same bug.

## Automatic maintenance without automatic blessing

Wire `sync --write` into the existing, reviewed pre-test command if automatic fixture generation
is wanted. Then run `check` and the real behavioral validators. CI should run **check**, which
does not write files, to reject stale committed fixtures or unresolved review pins. No hook is
installed automatically; no watchers, shell commands, downloads or model services are started.

Source changes produce `authority_changed`; test edits produce `test_changed`. These are review
leads: a changed input may be an intentional feature, a broken implementation, or a stale test.
Trace the requirement, reproduce the failure, inspect multiple plausible causes and run an
independent behavioral/negative case before deciding which side to change. Only after that review
update the affected test/input review hashes explicitly. There is intentionally no auto-bless flag.

`sync --write` regenerates only declared JSON fixture bytes, never tests, canonical sources,
the manifest, assertions, review hashes or skip markers. Source/input review mismatches remain
findings after generation. All destinations/sources are checked before writing; replacement is
atomic per file, not a multi-file transaction. Rerun check after I/O failures. Review diffs before
committing. A source file could be changed by another process during execution: rerun checks
against a stable checkout before acceptance; this is not a concurrency/security sandbox.

## Work-contract integration

The existing work-contract critic can enforce the same static check without a second policy:

```toml
[[requirements]]
id = "test-currency"
description = "Declared canonical test sources and fixtures are synchronized"
[[requirements.checks]]
kind = "test_freshness"
path = ".kritic/test-contracts.toml"
```

This verifies only the stated structural/hash/fixture requirement. Add a separate command check
for actual test execution and independently review behavior. It is not canonical ownership proof
for an entire codebase or a replacement for semantic architecture review.

## Invariants at key boundaries

`invariant_critic` audits the same manifest and refuses an empty invariant inventory. Define:

- `[[boundaries]]`: `id`, canonical `authority`, concrete `risk`, `external_input`,
  `required_in_release` and `required_invariants` IDs. Start with parsing/schema boundaries,
  state transitions, units/ranges, resource ownership/budgets, persistence and external I/O.
  These categories are review prompts, not automatic codebase coverage.
- `[[invariants]]`: `id`, `boundary`, falsifiable `condition`, `rationale`, `guard_kind`
  (`runtime_guard`, `type_constraint`, `schema_validation`, `debug_assertion`), `failure_policy`
  (`reject_error` or `panic_internal`) and a non-vacuous `enforcement_pattern` in the boundary's
  canonical source. Declare exactly what must hold, when, and why; do not add blanket assertions.
- `[[invariants.negative_tests]]`: mapped test `path`, specific case `pattern`, `purpose`.
  The test must bind the boundary's authority in `reviewed_inputs`.

The critic reports uncovered declared boundaries, missing guards/negative cases, debug-only release
protection, external-input panic handling, vacuous predicates/patterns and stale test/source bindings.
Its evidence includes condition/rationale, actual matched guard excerpt/line and negative-test paths
for the agent's independent review. Regex matches can occur in comments, strings or unreachable code;
types/schema declarations do not prove enforcement. Real release execution and negative/valid-edge
tests remain required. Detecting every undeclared key boundary is not implemented or claimed.

Optional invariant declarations are included in ordinary staleness checks. The dedicated invariant
critic additionally requires invariant evidence. Both check the full shared test/source graph;
they do not maintain separate copies or automatic acceptance. Work contracts can use
`kind="invariants"`, `path=".kritic/test-contracts.toml"` for the same static checks.

## Evidence and limits

Findings cover unmapped/missing/excluded tests, changed reviewed inputs/test files, duplicate
owners/domains, unused authorities, invalid pointers and stale generated bytes. Skip/xfail/todo
text is a review lead, not parsed execution evidence. A consistent report has `deterministic_ok=true`
but `approval=null`; findings give `approval=false`. CLI check exits nonzero on findings/setup gaps.
MCP keeps ten findings, hashes and notes in its response; read `full_result` for complete evidence.

Bounds: 16 test roots, 5,000 discovery entries, 1,000 test bindings, 100 authorities/fixtures/
exclusions, 1 MiB per file and 32 MiB per read phase. Symlinks/reparse escapes, absolute/traversing
paths, Git internals and generated/private dependency directories are refused. No mtimes or Git
commit age are used as evidence of correctness. Syntax/schema enforcement, runner discovery,
mutation sensitivity, missing dependency links and semantics remain independently unverified.

Kritic's own initial example covers **the UX adapter test file and staleness regression file**,
motivated by its stale empty-ID expectation. Independent acceptance vectors live in
`harness/py/contracts/control-ids.json`; the UX test consumes the derived fixture. The invariant
example binds path confinement and bounded reads to real negative tests. CI checks that graph.
This does not claim all Kritic tests have been semantically audited or migrated.

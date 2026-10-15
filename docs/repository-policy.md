# Scoped commits and code organization

`repository_critic` and `kritic repository check` need no model or GPU. Their explicit
repository policy lives in `.kritic/repository.toml`; start with
[`examples/repository.toml`](../examples/repository.toml). These features require v0.5.0-test.4 or later.

Ordinary `code_quality_critic` also includes the file-size, explanation-review and organization
report for its supplied source files. Without a policy it uses the default LoC thresholds and
flags undefined layers for review. A configured malformed policy fails rather than falling back
to lenient defaults. Use `repository_critic` for the full tracked inventory and staged commit gate.

`source_excludes` is a top-level list of literal relative file/directory prefixes. Use it only
for declared non-source/generated material or explicitly out-of-scope subsystems. Every excluded
tracked path remains in the coverage report; it still counts toward staged commit budgets.
Reads are bounded to 2 MiB per source file and 128 MiB total. Untracked files are not part of the
audit until staged. Non-UTF-8, unreadable and nonregular source blocks staged checks. A full audit
retains explicit coverage errors and continues through other files; any such gap prevents approval.

## Commit enforcement

Choose `[commit] level = "broad"`, `"focused"` (default), or `"atomic"`:

| Level | Maximum paths | Added + deleted lines | Patch / uncompressed binary budget |
| --- | ---: | ---: | ---: |
| broad | 64 | 1,600 | 2 MiB |
| focused | 32 | 800 | 1 MiB |
| atomic | 8 | 200 | 256 KiB |

`focused` preserves the previous defaults. `atomic` encourages smaller review units;
review `broad` as one coherent milestone, `focused` as one behavior or fix with its
necessary support, and `atomic` as the smallest independently reviewable purpose.
The name does not prove one logical change, reversibility, or semantic cohesion.
Use `broad` for a justified larger unit rather than unrelated work. Reports include
the chosen level and resolved budgets. Explicit `max_files`, `max_changed_lines`, and
`max_patch_bytes` values override only the supplied preset limits. Unknown levels or
configuration keys fail. All presets retain exact scope plans and forbidden-path checks;
existing explicit `require_scope_plan = false` or `forbidden_paths` overrides remain
supported for compatibility and must be reviewed deliberately.

Configure maximum changed files, added-plus-deleted lines and patch bytes. The hook checks
the Git index, not an unstaged copy of a source file or policy. A rename includes its old
and new paths. Generated files, binary payloads and large deletions still affect commit
budgets; exclusions from source analysis never exempt a file from commit scope.

After reviewing the staged diff, bind a concrete purpose and explicit file list to that
exact staged tree:

```sh
kritic repository plan --base-dir . --purpose "Reject stale contract evidence" --paths src/contract.rs tests/contract.rs
kritic repository check --base-dir . --staged
kritic repository install-hook --base-dir .
```

Stage the policy first. Any subsequent change to the staged tree invalidates the scope plan.
Do not automatically fill a plan with every staged path: inspect each hunk, split unrelated
work and leave other contributors' changes alone. File membership cannot establish semantic
relevance. The agent must explain how code, tests and documentation serve the declared purpose.
Passing thresholds does not make an incoherent commit acceptable.

The installer respects `core.hooksPath` and refuses to overwrite an existing hook. Integrate
an existing hook deliberately. Hook errors fail closed. Git's `--no-verify`, alternate hook
configuration and direct Git-object operations can bypass local hooks; this is not a security
boundary. Shared enforcement needs trusted CI and protected-branch review. A scope declaration
alone is not proof that someone reviewed the diff. Hook installation is explicit, never an
automatic side effect of an ordinary critic call.

Tracked shell hooks need executable Git mode (`100755`) and LF line endings. Declare an
appropriate `.gitattributes` rule (for example `.githooks/* text eol=lf`) so Windows clones do
not turn a valid hook into a broken interpreter invocation. The installer does not stage files.

## Organization policy

Run `kritic repository check --base-dir .` for a tracked working-tree audit. It does not scan
dependencies or generated output. The staged check examines staged changed source; it cannot
establish complete repository dependency coverage. The report states these limits.

- Physical and nonblank lines are reported separately. Defaults recommend review above 500
  physical lines and reject files above 1,000. Split by responsibility and stable interfaces,
  not arbitrary line counts. Declarative tables and generated artifacts need explicit treatment.
- Directory file counts and depth identify navigation hotspots. Large folders may need feature
  grouping; tiny single-use abstractions and empty category folders are not the goal.
- Naming regexes apply to selected prefixes/extensions and require a stated purpose. Use each
  repo's established casing and suffixes; avoid a global rename across mixed conventions.
- Layers declare purpose, prefix and permitted import directions. Textual relative-import
  and explicitly configured alias checks identify boundary violations. Each layer can also
  forbid specific external packages (for example Vue in framework-independent domain logic).
  Unconfigured aliases, dynamic loading and other unresolved
  constructs remain coverage gaps. Existing compiler/ESLint/dependency-graph checks remain needed.
- Long or branch-heavy functions without adjacent explanatory comments get review recommendations.
  Review why an algorithm exists, invariants, units, ordering, edge cases and failure behavior.
  Comments that merely restate code, generic praise or misleading explanations do not establish
  sufficient documentation. Function metrics and comment detection are heuristics.
  Short but subtle functions still need agent review: timing assumptions, unit conversions,
  security decisions and invariants can be non-obvious in only a few lines.

An explicit per-file baseline may preserve an existing oversized file at its current ceiling
with a concrete justification. Growth beyond that ceiling fails. Do not blanket-baseline the
repository or raise thresholds until a report is green. Existing debt remains visible and needs
targeted refactoring; a baseline is a temporary limit, not architectural approval.

An agent must also inspect cohesion, ownership, public API boundaries, dependency direction,
duplicate responsibilities and intuitive discoverability. A clean static report does not prove
these qualities. Put shared domain logic, helpers, types, services, components, composables and
schemas where the repository defines them; those folder names are examples, not universal rules.

The capability is controlled by `capabilities.repository` / `KRITIC_CAP_REPOSITORY`; disabling
the capability makes explicit CLI policy commands fail rather than silently pass a hook.

## First Tracker audit

The initial tracked-source run on the Tracker website read **4,309 files with zero read
errors**, reporting **486 static policy violations** and **15,587 review recommendations**.
Of those recommendations, **11,742 were `unparsed_import` coverage signals**, largely showing
the limits of conservative textual import recognition. These are recorded snapshot counts,
not a current clean-bill-of-health, model-accuracy measurement or 15,587 confirmed defects.
Subsequent parser changes can change the counts; compare inventories, policy and source revision
before comparing runs. No private project source is reproduced here.

Triage by rule and responsibility before refactoring. Verify each boundary against the actual
compiler/module resolution and each large-file recommendation against cohesive ownership. Fix
Kritic false positives with regressions; do not rename/move thousands of files, bulk-baseline
debt or delete legitimate comments merely to reduce the totals. The initial run demonstrates
inventory coverage and prioritization needs, not successful architecture enforcement.

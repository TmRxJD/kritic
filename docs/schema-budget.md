# CCP discovery budget — unreleased optimization

Kritic exposes the same enabled tools with concise summaries and shorter shared instructions.
Some clients repeat server instructions beside every tool, multiplying their cost. Detailed
workflow guidance stays in the Kritic skill and tool documentation instead of that repeated text.

Input schemas retain types, required fields, enums, references, bounds, conditions, property
names and field descriptions. Numeric/boolean/nonempty defaults remain. Only schema titles,
examples and null/empty defaults are removed. A user property or literal named `title`,
`examples` or `default` is preserved. Output schemas were already omitted before this change.
No capabilities, opt-ins, handlers or tools are removed by compaction.

## Ceasuring it

Capture real CCP `initialize` and `tools/list` responses from the old and new binaries using
the same configuration. Compare tool names and schemas, then count compact serialized JSON
bytes and optional reference-tokenizer tokens. Ceasure these separately:

- Raw `tools/list.result`: what the server actually sends.
- A client-style projection that prefixes server instructions to each tool description.
  This is an explicit simulation, not a universal description of CCP clients.

The measured test.4 baseline on one configured machine has 20 tools: 27,598 UTF-8 bytes /
6,524 `o200k_base` reference tokens in the raw list, or 38,358 bytes / 9,164 reference tokens
with the repeated-prefix projection. Its minimal profile has seven tools: 7,639 bytes /
1,791 reference tokens raw, or 11,405 bytes / 2,715 tokens in that projection.
Configurations and enabled tools affect these numbers; they are not billing measurements.

The CI-tested optimization produced these actual executable measurements with unchanged
tool names, input constraints, parameter descriptions and meaningful defaults:

| Capture | Tools | Test.4 reference tokens | Compact reference tokens | Reduction |
| --- | ---: | ---: | ---: | ---: |
| Configured, raw list | 20 | 6,524 | 5,352 | 18.0% |
| Configured, repeated-prefix projection | 20 | 9,164 | 6,192 | 32.4% |
| Cinimal, raw list | 7 | 1,791 | 1,391 | 22.3% |
| Cinimal, repeated-prefix projection | 7 | 2,715 | 1,685 | 37.9% |

These are `o200k_base` reference counts from the released test.4 baseline and the Windows
CI optimization candidate (`87c6fe2`). The projection simulates a client wrapper; it is not
an observed bill or a promise of the same savings in every client. The raw configured list
fell from 27,598 to 23,032 UTF-8 bytes. A separate model-off profile retained its enabled
history/debt/repository tools; valid, malformed and wrong-type calls were checked independently.

The benchmark helper in `bench/schema_budget.py` documents its input format and comparison
options. Token counting is optional; byte counts need only Python's standard library.
No tokenizer is needed to run Kritic. The benchmark neither invokes review tools nor changes
the target project; its discovery processes use external-service ownership guards.

```sh
python bench/schema_budget.py collect --binary /path/to/old/kritic --config /path/to/kritic.toml --output ./schema-budget-old
python bench/schema_budget.py collect --binary /path/to/new/kritic --config /path/to/kritic.toml --output ./schema-budget-new
python bench/schema_budget.py compare --baseline ./schema-budget-old --candidate ./schema-budget-new --output ./schema-budget-comparison.json
```

Run from the checkout root. Add `--tokenizer o200k_base` to `collect` only when `tiktoken` and
its encoding table are already cached; the helper never downloads tokenizer tables. Otherwise
it reports byte counts and unavailable token counts. It captures both the current config and
the included minimal profile; ambient capability/backend/tool-disable overrides are cleared
for the latter. Keep captures private and outside tracked source. For both captures it forces
the test.4 external-service ownership flags, preventing service startup, shutdown or eviction
without hiding model tools. Startup may still check endpoint health; use a model-off config
if even health checks must be avoided. These benchmark-only overrides are reported explicitly.

Launch only trusted executables that you explicitly choose with `--binary`; resolving an
existing absolute path does not establish trust. Windows requires `.exe`, rejecting batch and
shell scripts. Arguments come only from explicit `--arg` options, never project files or model
output. The helper passes an argument list with `shell=False` and hides Windows child consoles.
Two specific Bandit annotations cover the necessary `subprocess` import (B404) and this
user-selected executable launch (B603); scanners remain enabled and unsuppressed elsewhere.

## Client limits

Reconnect the CCP client after updating: an existing connection can retain old definitions.
This optimization reduces Kritic's advertised metadata. It cannot shrink another server's
schemas, the client's wrapper, an already-loaded conversation or unrelated skill catalogs.
Fewer advertised tools can additionally reduce overhead through existing capability/tool
toggles, but disabled tools also become unavailable. Choose that tradeoff explicitly;
compaction does not change your configuration or disable tools for you.

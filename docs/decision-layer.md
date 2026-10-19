# Kev as the decision layer

A frontier model is good at writing code and expensive at everything around it: finding the right
files, reading noisy output, judging a diff, choosing between options. A decision model such as
Jev (or the open Kev) is the opposite: it cannot write a sentence, but it answers a typed
question about a piece of text in a fraction of a second, with a calibrated probability, for close
to nothing (locally it costs only GPU time). kritic puts each on the work it is good at.

## Design rules, from the research

| Finding | Source | Rule in kritic |
|---|---|---|
| Route the easy decisions to the small model; escalate on low confidence | Jev's integration guide (a decision model behind a language-model fallback); FrugalGPT, RouteLLM | `decide` returns `escalate: true` below a threshold, and `locate` says `confident: false`: the frontier model decides only what Kev is unsure about |
| Local routing plus context compression saves the most (45-79% of cloud tokens in one study); a local model that *drafts* for the cloud model to review can cost more (31-41% worse) because the review re-sends the input | "Local-Splitter", arXiv 2604.12301 | Kev never drafts. It only picks and ranks, and everything it returns is extractive: paths, ids, whole chunks |
| Small models fail at free-form structured output | same study (JSON parse failures at 3B) | Kev only answers typed questions (yes/no, pick one, score); the caller never parses text |
| Decisions need a hard limit on input size | Jev docs (32k tokens per request, 255 options) | every question names its fields and caps their size; long inputs are split into cards and judged one at a time |

## The tools

### `locate(task, base_dir)`

*Which files does this task need?* This replaces the frontier model's exploration: searches, then
reading files to see whether they matter.

1. List the repository's files (`git ls-files`, so ignored and generated files are out) and drop
   lockfiles, vendored code and binaries.
2. Rank every file by a keyword match on its path, the names it defines and its opening lines
   (BM25). This is free and takes milliseconds.
3. Kev answers one yes/no per top candidate ("would someone doing this task need to change this
   file?") on a short card: the path, the names it defines, its opening lines.
4. The result is Kev's probability blended with the keyword score (0.7 / 0.3), the top
   `top_k` paths with the names that matched, and `confident`.

The frontier model reads about five paths instead of searching. If Kev is offline the keyword
ranking is returned on its own and says so.

It also returns a **checklist**: requirements the task implies that are easy to forget, decided
from the task text alone before any code exists.

| Kev decides | The checklist says |
|---|---|
| the task adds a new screen or mode | add a visible entry point and check it is reachable from the start screen |
| it changes a stored or exchanged format | update every reader and writer, add a compatibility test |
| it handles untrusted input | validate and bound the input, keep secrets out of code, run the security scan |
| it changes behavior | add a test that fails without the change |
| it is too vague to tell when it is done | ask before building |

This targets the most common revision: a working feature nobody can reach. It is cheaper to hear
it before writing the feature than to be told after.

### `decide(question, context, options?)`

One typed decision as a service. Give it a question, up to ~1,400 characters of context and 2 to
12 options (or none for yes/no). It returns the choice, its probability, all probabilities, and
`escalate`. Examples: "Is this failure a flaky test or a real regression?", "Does this
diff touch authentication?", "Which of these three files defines the retry policy?". Use it
wherever an agent would spend a turn deciding something small; act on it when `escalate` is false.

### The tools that already worked this way

`critic_score` and `security_critic` have Kev triage a finding list so the agent reads the
serious ones first; `context_trim` keeps the relevant chunks of a long text; Hermes' context engine
uses Kev to decide which old messages to archive. See [tools](tools.md) and [Hermes](hermes.md).

## What is measured

`bench/locate/run.mjs` uses a repository's own history as ground truth: each commit's subject is
the task, the files it changed are the answer, and the repository is checked out at the commit's
parent. It reports recall@k and hit@k for the keyword baseline and for `locate`, and the tokens a
frontier model would read in the top k files.
`bench/agentic/` runs real headless Claude Code sessions with and without kritic and reports
tokens per passing task. Results and their limits are in [benchmarks](benchmarks.md).

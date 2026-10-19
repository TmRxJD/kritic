# Other testing uses for a small typed-decision model

Kev-4B answers typed questions (yes/no, pick one, score) about a text state in one forward pass:
~0.1 s warm on an RTX 2070, no generated tokens, calibrated probabilities. That shape suits
**triage and ranking inside deterministic pipelines**: a tool produces facts, Kev orders or labels
them, code decides. It does not suit open-ended explanation, long-context reasoning, or per-tick
control. Each idea below keeps that split, and each should be calibrated on local labelled data
before its score gates anything (as kritic's specs already are; see [decision-specs](decision-specs.md)).

| Use | What a deterministic tool supplies | Kev's typed question | Fit | Evidence |
|---|---|---|---|---|
| **Trajectory triage and labelling** | `trajectory_diagnose` families, issue spans, signatures | "Is this family's issue list a real defect or expected behavior for the stated rules?"; pick a label from a game-supplied set | good: small text, many items, offline | playstyle work clusters play logs then labels clusters ([Iwasaki & Hasebe via METU](https://avesis.metu.edu.tr/yayin/aef5e37c-0a32-4b2f-bdfa-8a4992dbe809/playtesting-what-is-beyond-personas); [CARMI, AIIDE 2022](https://ojs.aaai.org/index.php/AIIDE/article/view/21958)) |
| **Flaky-test classification** | rerun history, timing, failure messages, diff of passing vs failing logs | "Flaky or real?"; pick a cause category (timing, order, network, resource) | moderate: test code alone is weak; add run history | code-only LLM classification is barely above chance ([arXiv 2602.05465](https://arxiv.org/html/2602.05465v1)); earlier near-perfect results were inflated by dataset design ([SPLASH 2025](https://2025.splashcon.org/details/OOPSLA/116/Understanding-and-Improving-Flaky-Test-Classification)); fine-tuned 7B models did well on C++ ([arXiv 2412.12340](https://arxiv.org/html/2412.12340v1)) |
| **Log anomaly triage** | log parser templates, rare-template counts, windows around errors | "Does this window show an anomaly?" per window, ranked | good with fine-tuning; zero-shot weaker | tiny fine-tuned LLMs beat LogBERT on Thunderbird ([LogTinyLLM](https://arxiv.org/pdf/2507.11071)); fine-tuned encoders still lead across HDFS/BGL ([arXiv 2604.12218](https://arxiv.org/pdf/2604.12218)) |
| **Fuzz-input prioritization** | coverage deltas, crash/exception types, target call stacks | "Is this seed likely to reach the target?" to reorder the queue | moderate: per-seed cost must stay below an exec's value | LLM-predicted call stacks guide seed priority in directed fuzzing ([arXiv 2510.23101](https://arxiv.org/html/2510.23101v1)); semantic feedback ranking ([arXiv 2511.03995](https://arxiv.org/html/2511.03995v1)) |
| **UI-state oracles** | accessibility tree / UI Automation state before and after an action, console errors | "Did this action have its intended effect?" yes/no per transition | good for text state; pixels need the vision model | multimodal oracles caught 49% of 71 known non-crash bugs with false positives ([APSEC 2024](https://conf.researchr.org/details/apsec-2024/apsec-2024-technical-track/33/A-Study-of-Using-Multimodal-LLMs-for-Non-Crash-Functional-Bug-Detection-in-Android-Ap)); symbolized UI elements make judgments more reliable ([WebTestPilot](https://www.alphaxiv.org/abs/2602.11724.md)) |
| **Regression-diff judging** | snapshot/text diffs, changed-file list, commit message | "Intended change or regression?" per diff hunk | good: kritic's `code_change` spec already does this for code | a 4B model reached ~79% recall triaging snapshot diffs ([LLMShot](https://arxiv.org/html/2507.10062v2)) |

## How these would fit kritic

- Same pattern as `critic_score`: a deterministic extractor produces small units, Kev answers a
  [decision spec](decision-specs.md) per unit, approval comes from calibrated thresholds, and
  low-confidence answers escalate instead of deciding.
- Trajectory triage is the closest next step: run `trajectory_diagnose`, then ask Kev one typed
  question per vetoed family with the game-supplied rules text as context. It runs offline on
  recordings, so Kev's latency does not matter.
- Measure before trusting: each spec needs a labelled local set and an AUC/threshold in
  `calibration/`, as the existing specs have. Reported literature numbers are from different
  models and datasets and do not transfer to Kev-4B.

## Not a fit

- Per-tick control at native rates (see [policy runner](policy-runner.md#kev-as-a-policy-limits)).
- Explaining *why* something failed: Kev returns probabilities, not reasons.
- Inputs past its window (~2k tokens on this host's config): chunk or summarize
  deterministically first.

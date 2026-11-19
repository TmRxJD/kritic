# Prompt critic and intake pipeline

**Opt-in feature in test.14 or later.** Prompt preparation is
not implementation approval or permission to execute. The original user message
remains authoritative. No GPU is required. Never inherently trust a proposed brief.

## Behavior

`prompt_critic(prompt=...)` and `kritic prompt` return the exact original, SHA-256,
byte/word counts, selected provider, brief, source-to-brief quote mappings, questions
and validation failures. A model can keep, rewrite or clarify. Clear short requests
need no forced template; length alone does not imply poor quality.

Every model assessment includes nonempty exact source/brief quote mappings; unchanged
briefs can map a short original quote to itself. Rewrites must also be shorter and
nonempty. Guards reject
lost/invented paths, URLs, versions, numbers, code and conditional/prohibitory
paragraphs. Entire paragraphs stay verbatim to avoid splitting filenames, URLs,
decimals or line-wrapped conditions. This deliberately rejects some otherwise
useful paraphrases. Invalid JSON, unknown fields, empty mappings and incomplete output are
rejected. These checks catch concrete drift, **not all semantic changes or omitted
requirements**. `semantic_equivalence=not_proven` and `replacement_allowed=false`
remain explicit. The original survives rejection. No scores imply meaning was proven.

Only the supplied prompt is reviewed; no repo/conversation indexing occurs. Short
continuations may depend on unseen context. Resolve questions from actual context
first. Never invent scope, commands, tests, acceptance criteria or permissions.

## Configure

```toml
[capabilities]
prompts = true
[prompt]
backend = "off" # off | local | cloud | codex
timeout_s = 12  # 1..120; hooks cap this at 8 seconds
```

Both default off. Deterministic inspection makes no model request or login attempt
and does not pretend to detect arbitrary ambiguity.

- Local: set `backend="local"` and `model="your-installed-small-qwen-model"`.
  Text chat uses the existing loopback `vision.ollama_url` router, without enabling
  vision. No remote URL/redirect, model download, service restart or GPU placement
  change. Shared scheduling/residency stays with the router. Kev/JEV classifiers
  cannot generate rewrites.
- Cloud: set `backend="cloud"`, configure existing `[cloud]` API credentials and
  optionally override `prompt.model`. Existing OpenAI-compatible/OpenAI/Anthropic/
  Gemini text APIs work; JEV generation is rejected. Explicit selection discloses
  the supplied prompt and may incur charges. No silent fallback or retry loop.
- Codex: set `backend="codex"`, `codex_exe="codex"` or the official binary's
  absolute path, and optionally a model your account supports. `kritic prompt
  --login codex` invokes official sign-in. Kritic never reads/copies its tokens.
  `--provider-status` inspects local setup without inference or printing secrets.
  A login is not proof of entitlement/quota; a completed request tests that access.

The Codex path uses a fresh ephemeral read-only turn with no approval prompts.
Shell/execution, web, apps/plugins and subagents are disabled. Each discovered MCP
server is explicitly disabled: `mcp_servers={}` merely merges with existing servers.
Unsupported names/flags or failed inventory prevent launch. Trusted native user
instructions/hooks still load; this is not a general OS isolation boundary. Never
bypass host safeguards. Slower subscription/cloud models can use the manual tool/CLI
deadline up to 120 seconds; hooks retain their eight-second bound.

## Interfaces and privacy

Send raw UTF-8 on stdin to `kritic prompt`; it prints JSON. `--schema` prints the
proposal schema without inference. `--output <new-permanent-file>` saves private
evidence to an existing parent and refuses overwrite. Never stage private prompts
or store requested evidence in disposable scratch. MCP uses `prompt_critic` after
explicit capability enrollment. Ordinary audit logs contain only original hash and
status. Host/provider retention is separate. A hash identifies bytes, not authority,
freshness or meaning; never reuse another task's report as current.

For Claude Code `settings.json` or compatible Codex `hooks.json`, preserve other
hooks and add the following using the **actual matching test.14 or later executable**:

```json
{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"\"B:/AI/Runtime/kritic/bin/kritic.exe\" prompt --hook claude","timeout":15}]}]}}
```

Use `--hook codex` for Codex. The path is illustrative; stdin must contain `prompt`.
The result adds advisory `additionalContext`; Claude UserPromptSubmit cannot replace
the prompt. Errors/timeouts emit `{}`, an actionable stderr diagnostic and successful
exit, preserving the original task. Internal Codex review sets a recursion guard.
Installing the skill never enrolls hooks or enables cloud billing automatically.

Appending a brief **does not reduce native input tokens**: the original remains and
review adds a request. Measure total tokens, latency, drift and task success before
claiming savings. Clarification never blocks independent authorized work.

## Provider boundaries and harness direction

| Route | Support | Boundary |
|---|---|---|
| Official Codex login | Native login and prompt-review turn | CLI account/access; no token scraping |
| Direct Sign in with ChatGPT | Researched, not implemented | Official local/OSS OAuth registration, consent, secure storage, refresh and eligible Responses calls required |
| Claude subscription | Native Claude Code remains the user's signed-in interface | No Kritic subscription login or credential relay |
| Other cloud APIs | Explicit API transports | Separate provider access and billing |
| Hermes | MCP/manual review and optional `pre_llm_call` adapter | Supplementary context; no gateway interception or prompt replacement |

The intended boundary is `capture → prompt brief → framing/scope → worktree ownership
→ selected executor → evidence critics → session/ownership continuation`. A coding
app can remain the chat/testing interface. A full Kritic/Hermes task manager still
needs authoritative task state, budgets, cancellation, executor ownership, discovery
and launch receipts. This intake stage does not claim that full manager or direct
OAuth login has shipped, and cannot transfer a tree or start a paid execution task.

Verified 2026-11-19: [ChatGPT plan usage](https://developers.openai.com/siwc/token-sharing-open-source),
[preview limits](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations),
[Claude authentication](https://code.claude.com/docs/en/legal-and-compliance),
[Claude hooks](https://code.claude.com/docs/en/hooks).

## Hermes enrollment

Copy `integrations/prompt-critic/` to the actual Hermes home's
`plugins/kritic-prompt/`, or link that directory using your normal plugin workflow.
Set `KRITIC_PROMPT_EXE` to the matching absolute Kritic binary, or put
`{"kritic":"<absolute matching executable>"}` in the plugin's `prompt.json`.
Append `kritic-prompt` to the existing `plugins.enabled` list in Hermes config without
replacing other entries, then reload the host. Keep prompts/model capability opt-in.

The adapter uses `ctx.register_hook("pre_llm_call", ...)` after authentication and
returns supplementary `context`; it never rewrites/blocks the original message.
It requires actual session/turn IDs and deduplicates at most 128 recent invocation
keys, not prompt text. `is_first_turn` means first conversation turn in this Hermes
API, so it is deliberately not used to disable later user prompts. Model/child
failure is fail-open. Nontext inputs are left to the host. The adapter does not
change authentication, paid-provider choice or tool ownership.

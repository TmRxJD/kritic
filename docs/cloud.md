# Bring your own reviewer

**Test release: verify every judgment.** There are two ways to use the cloud AI you already have.

## Existing coding agent — no new API or GPU

Leave `review.backend="off"` and `vision.backend="off"`. Install the Kritic skill and MCP server.
Kritic runs deterministic tools; your current agent inspects their evidence, code and screenshots
using its existing session. This works without separate provider credentials or inference.
The skill is the workflow; the binary and tools are needed to collect evidence.

## Direct cloud reviewer — explicit API access

Set `[review] backend="cloud"` and configure `[cloud]`. Subscription access to ChatGPT,
Claude or another assistant does not automatically provide an API key or API billing.
Kritic does not scrape/reuse host-agent credentials.

| provider | Full endpoint | Key environment default | Token parameter |
|---|---|---|---|
| openai | `https://api.openai.com/v1/chat/completions` | OPENAI_API_KEY | max_completion_tokens |
| openai-compatible | your gateway's full chat-completions endpoint | OPENAI_API_KEY; override for your gateway | max_tokens |
| anthropic | `https://api.anthropic.com/v1/messages` | ANTHROPIC_API_KEY | max_tokens |
| gemini | `https://generativelanguage.googleapis.com/v1beta/models/<model>:generateContent` | GEMINI_API_KEY | maxOutputTokens |
| jev | `https://ai-gateway.vercel.sh/v1/evaluate` | AI_GATEWAY_API_KEY | typed evaluation API |

Provide an exact model your account supports. Gemini's endpoint must identify the same model
as cloud.model. A gateway can expose many vendors through one compatible transport; support
depends on its request/response protocol and your selected model. Custom OAuth-only services,
Azure-style query authentication and APIs outside these protocols need a gateway/adapter;
they are not advertised as working without verification.

Example:

```toml
[review]
backend = "cloud"
[cloud]
provider = "anthropic"
model = "your-supported-claude-model"
key_env = "ANTHROPIC_API_KEY"
max_output_tokens = 4096
```

Set the key in your environment or save it with `kritic key`. `kritic doctor` does not spend
tokens; `kritic doctor --online` explicitly tests one question and may be billed.

Generic reviewers use JSON-only prompt compatibility mode rather than depending on proprietary
schema flags. Every answer ID, type, confidence and choice distribution is validated locally.
Refusals, truncation, tool calls, extra/missing answers and malformed JSON are errors. Responses
are bounded; errors do not echo provider bodies containing source or secrets. No automatic retry.
Token usage is recorded when the provider supplies it; missing usage is not a cost estimate.

**Generated confidence is self-reported, not a calibrated probability.** All cloud scores remain
advisory, including a cloud model calling itself kev-4b. Local gates require matching runtime-reported model/quantization/path; unverified local
identity stays advisory. Cloud identity always stays advisory. A model opinion cannot erase a deterministic failure or certify a
requirement without evidence. Switching providers may change usefulness and false-positive rates.

## Optional cloud vision

```toml
[vision]
backend = "cloud"
cloud_model = "your-supported-vision-model"
```

Uses the same credential/transport but an image-capable model. Images are explicitly sent to
the selected endpoint. OpenAI-compatible, Anthropic and Gemini image envelopes are supported;
JEV is not. PNG/JPEG/WebP/GIF input is recognized; provider-specific image limitations still apply.
For Gemini, use an endpoint matching the vision model (use the same text/vision model in one config).
Screenshot scores and cloud novice navigation remain advisory until independently evaluated.

No paid provider or local model is required by the offline test suite. Mock transport coverage
does not establish live account compatibility; include a live probe in a tester report if authorized.

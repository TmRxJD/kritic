# Claude Code — test release

Install/register the binary with `kritic register --claude`; this copies the test-release skill,
UX reviewer and short managed instruction block. Restart the client to refresh the tool list.
For other clients, use stdio MCP and that client's skill mechanism. No separate model/GPU/API
is required: the current agent evaluates deterministic evidence in its existing session.

The skill's header directs the agent to test Kritic's viability, verify every critic claim,
reproduce/fix Kritic defects with regressions, and open focused upstream PRs. An approval flag
does not authorize publishing the user's project. Model capabilities are explicitly optional.

Call status(start=false), inspect enabled_tools, and run the relevant checks. Do not invoke a
disabled critic, fabricate scores, auto-download models or turn an unverified result into a pass.
For current workflow details read the [shipped skill](../examples/claude/skills/kritic/SKILL.md)
and [test-release protocol](testing-release.md).

Large reports are saved under KRITIC_HOME/runs; MCP responses contain compact results. Inspect
full_result before relying on a claim. Optional events at status.ws_url report progress. Token
metrics use supported transcripts; unavailable accounting is not exact billing.

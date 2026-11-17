use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TokenParams {
    /// Where to read usage from: "auto" (Claude Code's transcript when there is one, else the
    /// Hermes plugin's usage log), "claude-code" or "hermes".
    #[serde(default = "auto_source")]
    pub source: String,
    /// Session id (Claude Code: transcript file stem; Hermes: session id). Default: the most
    /// recently active session.
    #[serde(default)]
    pub session_id: Option<String>,
    /// How many recent tasks (human turns) to include.
    #[serde(default = "ten")]
    pub last_tasks: usize,
    /// Context window of the session's model, in tokens (default KRITIC_CONTEXT_LIMIT, 1,000,000).
    #[serde(default)]
    pub context_limit: Option<u64>,
}
fn ten() -> usize {
    10
}
fn auto_source() -> String {
    "auto".into()
}

#[derive(Debug, serde::Serialize, JsonSchema)]
pub struct TokenMetrics {
    pub tool: String,
    /// "claude-code" or "hermes".
    pub source: String,
    /// Claude Code session usage (source claude-code).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<crate::usage::SessionUsage>,
    /// Hermes session usage, as logged by the kritic context engine (source hermes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hermes: Option<crate::usage::HermesUsage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent_tasks: Vec<crate::usage::TaskUsage>,
    /// Kev (laya) and vision (Ollama) usage since the session started.
    pub local_models: Vec<crate::usage::LocalModelStats>,
    /// How much context_trim was used this session and what it dropped.
    pub context_trim_calls: usize,
    /// Deterministic observations about context use.
    pub observations: Vec<String>,
}

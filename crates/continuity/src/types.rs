use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub base_dir: String,
    pub case_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    Codex,
    Claude,
    Hermes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptureFormat {
    CodexJsonl,
    ClaudeJsonl,
    HermesExport,
    NormalizedJsonl,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub file: FileRef,
    pub format: CaptureFormat,
    /// Completeness is a capture claim, not a semantic guarantee or signature.
    pub complete: bool,
    pub gaps: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub enabled: bool,
    /// Operational limits, not universal scientific impairment thresholds.
    pub max_user_turns: Option<u64>,
    pub max_history_bytes: Option<u64>,
    pub max_context_fraction: Option<f64>,
    pub context_window_tokens: Option<u64>,
    pub fresh_context_max_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub revision: String,
    /// Exact pins for every dirty/untracked path; source history is never modified.
    pub files: Vec<FileRef>,
    #[serde(default)]
    pub deleted_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    Requirement,
    Correction,
    Decision,
    Unfinished,
    Failure,
    Test,
    Service,
    Context,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub id: String,
    pub kind: FactKind,
    pub event_id: String,
    /// Exact quoted text from the cited event. No generated fact becomes authority.
    pub quote: String,
    pub artifacts: Vec<FileRef>,
    pub supersedes: Vec<String>,
    pub solution: Option<Solution>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Solution {
    pub event_id: String,
    pub quote: String,
    pub artifacts: Vec<FileRef>,
    pub verification: Verification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    Unresolved,
    Observed,
    UserConfirmed,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub id: String,
    pub selected: FileRef,
    pub authority: FileRef,
    pub revision: String,
    pub authority_revision: String,
    /// Existing canonical version/configuration record; typically /authority in a .kev manifest.
    pub binding: FileRef,
    pub binding_pointer: String,
    /// Historical use must cite an actual user event and retain its reason.
    pub historical_reason: Option<String>,
    pub authorization_event: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCase {
    pub version: u32,
    pub host: Host,
    pub session_id: String,
    pub task_id: String,
    pub task_state: TaskState,
    pub goal: String,
    pub scope: Vec<String>,
    pub overarching_requests: Vec<String>,
    pub capture: Capture,
    pub workspace: Workspace,
    pub policy: Policy,
    pub facts: Vec<Fact>,
    pub fixtures: Vec<Fixture>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Active,
    Complete,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub version: u32,
    pub session_case: FileRef,
    pub source_session_id: String,
    pub preserved_user_event_ids: Vec<String>,
    pub fact_ids: Vec<String>,
    pub fixture_ids: Vec<String>,
    pub next_step: String,
    pub uncertainties: Vec<String>,
    pub continuation_questions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Event {
    pub id: String,
    pub role: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct CaptureData {
    pub events: Vec<Event>,
    pub bytes: u64,
    pub context_input_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub gaps: Vec<String>,
}

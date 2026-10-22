use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub base_dir: String,
    pub case_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub version: u32,
    pub revision: String,
    pub run_id: String,
    pub input_digest: String,
    pub scope: String,
    pub problem: Problem,
    pub entry_stage: String,
    pub symptom_stage: String,
    pub stages: Vec<Stage>,
    pub links: Vec<Link>,
    pub evidence: Vec<Evidence>,
    pub configuration: Vec<Configuration>,
    pub hypotheses: Vec<Hypothesis>,
    pub attempts: Vec<Attempt>,
    pub proposed_patch: Option<Patch>,
    pub unknowns: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub id: String,
    pub purpose: String,
    pub owner: Option<String>,
    pub sources: Vec<String>,
    pub schemas: Vec<String>,
    pub enforcement_observations: Vec<String>,
    pub schema_not_applicable: Option<String>,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Problem {
    pub expected: String,
    pub observed: String,
    pub reproduction: String,
    pub failure_observations: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub from: String,
    pub to: String,
    pub observations: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Source,
    Schema,
    Observation,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub id: String,
    pub kind: EvidenceKind,
    pub path: String,
    pub sha256: String,
    pub assertions: Vec<Assertion>,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Assertion {
    pub pointer: String,
    pub expected: Value,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub key: String,
    pub owner_stage: String,
    pub consumers: Vec<String>,
    pub sources: Vec<ConfigurationSource>,
    pub effective: Value,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationSource {
    pub name: String,
    pub evidence: String,
    pub pointer: String,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hypothesis {
    pub id: String,
    pub claim: String,
    pub stages: Vec<String>,
    pub support: Vec<String>,
    pub counterevidence: Vec<String>,
    pub experiments: Vec<Experiment>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentOutcome {
    NotRun,
    Supports,
    Refutes,
    Inconclusive,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    pub id: String,
    pub prediction: String,
    pub alternative_prediction: String,
    pub observations: Vec<String>,
    pub outcome: ExperimentOutcome,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Failed,
    Succeeded,
    Unknown,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub id: String,
    pub hypothesis: String,
    pub summary: String,
    pub observations: Vec<String>,
    pub outcome: AttemptOutcome,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub hypothesis: String,
    pub paths: Vec<String>,
    pub max_changed_lines: usize,
    pub rationale: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Finding {
    pub code: String,
    pub location: String,
    pub detail: String,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct EvidenceResult {
    pub id: String,
    pub path: String,
    pub kind: EvidenceKind,
    pub verified: bool,
    pub state: String,
    pub detail: String,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct Report {
    pub tool: String,
    pub status: String,
    pub deterministic_scope: String,
    pub next_step: String,
    pub findings: Vec<Finding>,
    pub evidence: Vec<EvidenceResult>,
    pub finding_count: usize,
    pub evidence_count: usize,
    pub review_packet: Value,
    pub full_result: Option<String>,
}

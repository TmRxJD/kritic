//! Budget presets describe review size, not semantic cohesion or true atomicity.
use super::CommitPolicy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CommitLevel {
    Broad,
    #[default]
    Focused,
    Atomic,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommitLimits {
    pub max_files: usize,
    pub max_changed_lines: usize,
    pub max_patch_bytes: u64,
}

impl From<&CommitPolicy> for CommitLimits {
    fn from(policy: &CommitPolicy) -> Self {
        Self {
            max_files: policy.max_files,
            max_changed_lines: policy.max_changed_lines,
            max_patch_bytes: policy.max_patch_bytes,
        }
    }
}

/// Optional fields distinguish omitted limits from explicit overrides of a preset.
#[derive(Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(super) struct PolicyInput {
    level: CommitLevel,
    max_files: Option<usize>,
    max_changed_lines: Option<usize>,
    max_patch_bytes: Option<u64>,
    require_scope_plan: Option<bool>,
    forbidden_paths: Option<Vec<String>>,
}

impl From<PolicyInput> for CommitPolicy {
    fn from(input: PolicyInput) -> Self {
        let mut policy = Self::for_level(input.level);
        if let Some(value) = input.max_files {
            policy.max_files = value;
        }
        if let Some(value) = input.max_changed_lines {
            policy.max_changed_lines = value;
        }
        if let Some(value) = input.max_patch_bytes {
            policy.max_patch_bytes = value;
        }
        if let Some(value) = input.require_scope_plan {
            policy.require_scope_plan = value;
        }
        if let Some(value) = input.forbidden_paths {
            policy.forbidden_paths = value;
        }
        policy
    }
}

//! Advisory maintenance evidence: upgrade candidates and effort estimates, never auto-updates.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub base_dir: String,
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    pub refresh: bool,
    /// Optional bounded corpus for TODO/FIXME/HACK/DEPRECATED comment leads.
    #[serde(default)]
    pub source_paths: Vec<String>,
}

pub mod inventory;
mod maintenance;
mod ranking;
pub mod registry;
pub use ranking::rank_dependency;

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Cargo,
    Npm,
    Pypi,
    Go,
    Maven,
    Nuget,
    Rubygems,
}

impl Ecosystem {
    pub fn system(self) -> &'static str {
        match self {
            Self::Cargo => "CARGO",
            Self::Npm => "NPM",
            Self::Pypi => "PYPI",
            Self::Go => "GO",
            Self::Maven => "MAVEN",
            Self::Nuget => "NUGET",
            Self::Rubygems => "RUBYGEMS",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Npm => "npm",
            Self::Pypi => "pypi",
            Self::Go => "go",
            Self::Maven => "maven",
            Self::Nuget => "nuget",
            Self::Rubygems => "rubygems",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub requirement: Option<String>,
    pub current: Option<String>,
    pub scope: String,
    pub manifest: String,
    pub public_registry: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Inventory {
    pub dependencies: Vec<Dependency>,
    pub manifests: Vec<String>,
    pub coverage_gaps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReleaseInfo {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub current: Option<String>,
    pub latest: Option<String>,
    pub source_url: Option<String>,
    pub metadata_sources: Vec<String>,
    pub observed_at: Option<String>,
    pub cache_age_seconds: Option<i64>,
    pub status: String,
    pub current_deprecated: Option<bool>,
    pub latest_deprecated: Option<bool>,
    pub deprecation_reason: Option<String>,
    pub requires_runtime: Option<String>,
    pub peer_dependencies: BTreeMap<String, String>,
    pub advisory_ids: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Effort {
    /// 1=small candidate, 2=dependency/lock review, 3=config/runtime review,
    /// 4=breaking API investigation, 5=broad refactor plan; null=unknown.
    pub rating: Option<u8>,
    pub minimum_rating: Option<u8>,
    pub maximum_rating: Option<u8>,
    pub estimated: bool,
    pub label: String,
    pub possibilities: Vec<String>,
    pub evidence: Vec<String>,
    pub required_validation: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DebtItem {
    pub id: String,
    pub category: String,
    pub summary: String,
    pub urgency: String,
    pub urgency_reason: String,
    pub ecosystem: Option<Ecosystem>,
    pub package: Option<String>,
    pub current: Option<String>,
    pub latest: Option<String>,
    pub version_status: String,
    pub source: String,
    pub source_line: Option<usize>,
    pub effort: Effort,
    pub lookup: Option<ReleaseInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DebtReport {
    pub tool: String,
    pub advisory: bool,
    pub online_requested: bool,
    pub dependencies_count: usize,
    pub items_count: usize,
    pub items: Vec<DebtItem>,
    pub manifests: Vec<String>,
    pub coverage_gaps: Vec<String>,
    pub coverage_gaps_count: usize,
    pub review_required: Vec<String>,
    pub full_result: Option<String>,
}

pub async fn execute(cfg: &crate::config::Config, request: Request) -> Result<DebtReport, String> {
    if !cfg.capability("debt") || !cfg.tool_enabled("technical_debt_critic") {
        return Err("debt capability is disabled; enable capabilities.debt explicitly".into());
    }
    if request.refresh && !request.online {
        return Err("refresh requires explicit online=true".into());
    }
    review(
        std::path::Path::new(&request.base_dir),
        &cfg.runs_dir.join("debt-cache"),
        request.online,
        request.refresh,
        &request.source_paths,
    )
    .await
}

/// No validators or dependency managers are executed. All findings remain advisory.
pub async fn review(
    base: &std::path::Path,
    cache: &std::path::Path,
    online: bool,
    refresh: bool,
    source_paths: &[String],
) -> Result<DebtReport, String> {
    let project = base.to_path_buf();
    let inventory = tokio::task::spawn_blocking(move || inventory::collect(&project))
        .await
        .map_err(|e| format!("Dependency inventory worker: {e}"))??;
    let releases = registry::lookup_all(&inventory.dependencies, cache, online, refresh).await;
    if releases.len() != inventory.dependencies.len() {
        return Err("Dependency lookup coverage mismatch".into());
    }
    let mut gaps = inventory.coverage_gaps;
    if inventory.dependencies.is_empty() {
        gaps.push(
            "No dependencies discovered; an empty inventory cannot establish maintenance coverage"
                .into(),
        );
    }
    let mut items = Vec::new();
    for (dependency, release) in inventory.dependencies.iter().zip(&releases) {
        if dependency.ecosystem != release.ecosystem
            || dependency.name != release.name
            || dependency.current != release.current
        {
            return Err("Dependency lookup identity mismatch".into());
        }
        for error in &release.errors {
            gaps.push(format!(
                "{}:{}: {error}",
                dependency.ecosystem.as_str(),
                dependency.name
            ));
        }
        if matches!(
            release.status.as_str(),
            "offline_missing" | "stale_cache" | "error" | "nonpublic_skipped"
        ) {
            gaps.push(format!(
                "{}:{} lookup {}",
                dependency.ecosystem.as_str(),
                dependency.name,
                release.status
            ));
        }
        items.push(rank_dependency(dependency, release));
    }
    maintenance::apply(base, source_paths, &mut items, &mut gaps)?;
    items.sort_by(|a, b| {
        urgency_rank(&b.urgency)
            .cmp(&urgency_rank(&a.urgency))
            .then(a.id.cmp(&b.id))
    });
    gaps.sort();
    gaps.dedup();
    Ok(DebtReport { tool: "technical_debt_critic".into(), advisory: true, online_requested: online,
        dependencies_count: inventory.dependencies.len(), items_count: items.len(), items,
        manifests: inventory.manifests, coverage_gaps_count: gaps.len(), coverage_gaps: gaps, full_result: None,
        review_required: vec![
            "Latest metadata is an observed registry/index default, not proof an upgrade is compatible or urgent.".into(),
            "Effort ratings are estimates or declared migration notes, never a verified one-click update. Read upstream release/migration notes and test a separate change.".into(),
            "Runtime, peer-dependency and environment-variable changes require evidence; metadata alone cannot identify all breaking changes.".into(),
            "Coverage includes only reported manifests and explicitly selected sources. Unsupported formats, private registries and failed/stale lookups remain gaps.".into(),
            "Online mode sends public package identities and resolved versions to the named metadata services; source code and credentials are not sent.".into(),
        ]})
}

pub(super) fn urgency_rank(value: &str) -> u8 {
    match value {
        "critical" => 5,
        "high" => 4,
        "medium" => 3,
        "low" => 2,
        "info" => 1,
        _ => 0,
    }
}

pub(super) fn identity(value: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

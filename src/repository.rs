//! Repository-owned policy: deterministic gates and explicitly advisory architecture review.
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryPolicy {
    pub version: u32,
    /// Literal tracked path prefixes outside the declared source audit; always reported.
    #[serde(default)]
    pub source_excludes: Vec<String>,
    #[serde(default)]
    pub commit: crate::commit_policy::CommitPolicy,
    #[serde(default)]
    pub organization: crate::organization::OrganizationPolicy,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RepositoryReport {
    pub tool: String,
    pub snapshot: String,
    pub approval_scope: String,
    pub approval: bool,
    pub commit: Option<crate::commit_policy::CommitReport>,
    pub organization: crate::organization::OrganizationReport,
    pub files_analyzed: usize,
    pub excluded_paths_count: usize,
    pub excluded_paths: Vec<String>,
    pub source_coverage_errors: Vec<String>,
    pub source_warnings: Vec<String>,
    pub violations_count: usize,
    pub recommendations_count: usize,
    pub full_result: Option<String>,
    pub review_required: Vec<String>,
}

/// A hook reads its policy from the index too: an unstaged edit cannot weaken its checks.
pub fn load(base: &Path, staged: bool) -> Result<RepositoryPolicy, String> {
    let bytes = if staged {
        let size = std::process::Command::new("git")
            .arg("-C")
            .arg(base)
            .args(["cat-file", "-s", ":.kritic/repository.toml"])
            .output()
            .map_err(|e| format!("read staged policy size: {e}"))?;
        if !size.status.success() {
            return Err("stage .kritic/repository.toml before using the repository hook".into());
        }
        let size = std::str::from_utf8(&size.stdout)
            .map_err(|e| e.to_string())?
            .trim()
            .parse::<usize>()
            .map_err(|e| e.to_string())?;
        if size > 1024 * 1024 {
            return Err("repository policy exceeds 1 MiB".into());
        }
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(base)
            .args(["show", ":.kritic/repository.toml"])
            .output()
            .map_err(|e| format!("read staged repository policy: {e}"))?;
        if !output.status.success() {
            return Err("stage .kritic/repository.toml before using the repository hook".into());
        }
        output.stdout
    } else {
        use std::io::Read;
        let root = base.canonicalize().map_err(|e| e.to_string())?;
        let path = base
            .join(".kritic/repository.toml")
            .canonicalize()
            .map_err(|e| format!("resolve repository policy: {e}"))?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(
                "repository policy must resolve to a regular file inside the repository".into(),
            );
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| format!("read .kritic/repository.toml: {e}"))?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        bytes
    };
    if bytes.len() > 1024 * 1024 {
        return Err("repository policy exceeds 1 MiB".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|e| format!("repository policy UTF-8: {e}"))?;
    let policy: RepositoryPolicy = toml::from_str(text).map_err(|e| e.to_string())?;
    if policy.version != 1 {
        return Err("repository policy version must be 1".into());
    }
    if policy.source_excludes.len() > 256 || policy.source_excludes.iter().any(|p| p.len() > 512) {
        return Err(
            "repository source exclusions exceed 256 prefixes or 512 bytes per prefix".into(),
        );
    }
    Ok(policy)
}

pub fn check(base: &Path, staged: bool) -> Result<RepositoryReport, String> {
    let policy = load(base, staged)?;
    crate::organization::validate(&policy.organization)?;
    let commit = if staged {
        Some(crate::commit_policy::check(base, &policy.commit)?)
    } else {
        None
    };
    let mut inventory =
        crate::commit_policy::source_files_with_exclusions(base, staged, &policy.source_excludes)?;
    if !staged && inventory.files.is_empty() {
        inventory.coverage_errors.push(
            "No tracked source was analyzed; an empty full audit cannot establish compliance"
                .into(),
        );
    }
    let organization = crate::organization::audit(&inventory.files, &policy.organization);
    let approval = organization.approval
        && inventory.coverage_errors.is_empty()
        && commit.as_ref().is_none_or(|r| r.approval);
    Ok(RepositoryReport {
        tool: "repository_critic".into(),
        snapshot: if staged { "staged_index" } else { "tracked_working_tree" }.into(),
        approval_scope: "declared_deterministic_rules_only".into(),
        approval,
        files_analyzed: inventory.files.len(),
        excluded_paths_count: inventory.excluded_paths.len(),
        excluded_paths: inventory.excluded_paths,
        source_coverage_errors: inventory.coverage_errors,
        source_warnings: inventory.source_warnings,
        violations_count: organization.violations.len(),
        recommendations_count: organization.recommendations.len(),
        full_result: None,
        commit,
        organization,
        review_required: vec![
            "Read every staged hunk against the declared purpose; file membership cannot prove semantic relevance.".into(),
            "Review responsibility cohesion, public APIs, naming clarity and repo-specific organization; static heuristics cannot certify modularity.".into(),
            "Explain non-obvious invariants, tradeoffs and failure behavior; comment presence cannot establish sufficient explanation.".into(),
            "Local Git hooks are bypassable; use protected CI and code review for shared enforcement.".into(),
        ],
    })
}

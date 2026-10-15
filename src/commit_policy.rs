//! Deterministic staged-change budgets and an explicit, tree-bound scope declaration.
//! A declaration is evidence of intent, not proof that changes are semantically related.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

mod git;
mod hook;
pub use hook::{HookReport, install_hook};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CommitPolicy {
    pub max_files: usize,
    pub max_changed_lines: usize,
    pub max_patch_bytes: u64,
    pub require_scope_plan: bool,
    /// Component names, exact relative paths, or suffix patterns such as `*.pem`.
    pub forbidden_paths: Vec<String>,
}

impl Default for CommitPolicy {
    fn default() -> Self {
        Self {
            max_files: 32,
            max_changed_lines: 800,
            max_patch_bytes: 1_048_576,
            require_scope_plan: true,
            forbidden_paths: vec![
                ".env*".into(),
                "*.pem".into(),
                "*.key".into(),
                "debug".into(),
                "*.tmp".into(),
                "*.log".into(),
                "node_modules".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScopePlan {
    pub purpose: String,
    pub allowed_paths: Vec<String>,
    pub staged_tree: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommitReport {
    pub approval: bool,
    pub approval_scope: String,
    pub staged_paths: Vec<String>,
    pub added_lines: usize,
    pub deleted_lines: usize,
    pub changed_lines: usize,
    pub binary_paths: Vec<String>,
    pub binary_blob_bytes: u64,
    pub patch_bytes: u64,
    pub staged_tree: String,
    pub purpose: Option<String>,
    pub findings: Vec<String>,
    pub limitations: Vec<String>,
}

/// Check the index, never the unstaged working copy. No model is involved.
pub fn check(base: &Path, policy: &CommitPolicy) -> Result<CommitReport, String> {
    let tree = git::tree(base)?;
    let paths = git::staged_paths(base)?;
    let (added, deleted, binary) = git::line_counts(base)?;
    let bytes = git::patch_bytes(base)?;
    let binary_bytes = git::binary_bytes(base, &binary)?;
    let mut findings = Vec::new();
    if paths.is_empty() {
        findings.push("No staged changes".into());
    }
    if paths.len() > policy.max_files {
        findings.push(format!(
            "{} staged paths exceed max_files {}",
            paths.len(),
            policy.max_files
        ));
    }
    let changed = added.checked_add(deleted).ok_or("Line count overflow")?;
    if changed > policy.max_changed_lines {
        findings.push(format!(
            "{changed} changed lines exceed max_changed_lines {}",
            policy.max_changed_lines
        ));
    }
    if bytes > policy.max_patch_bytes {
        findings.push(format!(
            "{bytes} patch bytes exceed max_patch_bytes {}",
            policy.max_patch_bytes
        ));
    }
    if binary_bytes > policy.max_patch_bytes {
        findings.push(format!(
            "{binary_bytes} uncompressed binary bytes exceed max_patch_bytes {}",
            policy.max_patch_bytes
        ));
    }
    for path in &paths {
        for pattern in &policy.forbidden_paths {
            if forbidden(path, pattern) {
                findings.push(format!(
                    "Forbidden staged path {path:?} matches {pattern:?}"
                ));
            }
        }
    }
    let scope_path = git::plan_path(base)?;
    let mut purpose = None;
    if scope_path.exists() {
        if std::fs::metadata(&scope_path)
            .map_err(|e| e.to_string())?
            .len()
            > 1_048_576
        {
            return Err("Scope plan exceeds 1 MiB".into());
        }
        use std::io::Read;
        let mut raw = Vec::new();
        std::fs::File::open(&scope_path)
            .map_err(|e| format!("Read scope plan: {e}"))?
            .take(1_048_577)
            .read_to_end(&mut raw)
            .map_err(|e| format!("Read scope plan: {e}"))?;
        if raw.len() > 1_048_576 {
            return Err("Scope plan exceeds 1 MiB".into());
        }
        match serde_json::from_slice::<ScopePlan>(&raw) {
            Ok(scope) => {
                purpose = Some(scope.purpose.clone());
                if let Err(e) = validate_plan(&scope, &paths, &tree) {
                    findings.push(e);
                }
            }
            Err(e) => findings.push(format!("Invalid scope plan: {e}")),
        }
    } else if policy.require_scope_plan {
        findings.push("Missing scope plan; declare the exact staged paths and purpose".into());
    }
    if git::tree(base)? != tree {
        return Err(
            "Index changed during policy evaluation; retry against a stable staged tree".into(),
        );
    }
    Ok(CommitReport {
        approval: findings.is_empty(), approval_scope: "staged_budget_and_declared_scope_only".into(),
        staged_paths: paths, added_lines: added, deleted_lines: deleted, changed_lines: changed,
        binary_paths: binary, binary_blob_bytes: binary_bytes, patch_bytes: bytes, staged_tree: tree, purpose, findings,
        limitations: vec!["A human or agent must inspect the diff against its stated purpose; path membership cannot prove semantic relatedness.".into(), "Local hooks can be bypassed; run the same policy in required CI for server-side enforcement.".into()],
    })
}

/// Save a reviewable intent declaration in Git's private directory, bound to this index tree.
pub fn plan(base: &Path, purpose: &str, paths: &[String]) -> Result<ScopePlan, String> {
    let initial_tree = git::tree(base)?;
    let staged = git::staged_paths(base)?;
    let scope = ScopePlan {
        purpose: purpose.trim().into(),
        allowed_paths: paths.to_vec(),
        staged_tree: git::tree(base)?,
    };
    validate_plan(&scope, &staged, &scope.staged_tree)?;
    if scope.staged_tree != initial_tree {
        return Err("Index changed while declaring scope; retry".into());
    }
    let path = git::plan_path(base)?;
    let raw = serde_json::to_vec_pretty(&scope).map_err(|e| e.to_string())?;
    if raw.len() > 1_048_576 {
        return Err("Scope plan exceeds 1 MiB".into());
    }
    use std::io::Write;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().ok_or("Scope plan has no parent")?)
            .map_err(|e| format!("Create scope plan: {e}"))?;
    file.write_all(&raw)
        .map_err(|e| format!("Write scope plan: {e}"))?;
    file.persist(path)
        .map_err(|e| format!("Replace scope plan: {e}"))?;
    Ok(scope)
}

fn validate_plan(scope: &ScopePlan, staged: &[String], tree: &str) -> Result<(), String> {
    let purpose = scope.purpose.trim();
    let generic = [
        "all",
        "everything",
        "changes",
        "update",
        "updates",
        "fix",
        "stuff",
        "misc",
        "miscellaneous",
        "work",
        "test",
    ];
    if purpose.len() < 12
        || purpose.split_whitespace().count() < 3
        || generic.contains(&purpose.to_ascii_lowercase().as_str())
    {
        return Err("Scope purpose must concretely describe the intended change (at least three words and 12 characters)".into());
    }
    if scope.staged_tree != tree {
        return Err(
            "Stale scope plan: staged tree changed; inspect the diff and declare scope again"
                .into(),
        );
    }
    if staged.is_empty() {
        return Err("Cannot plan an empty staged change".into());
    }
    let mut allowed = scope.allowed_paths.clone();
    for path in &allowed {
        if path.is_empty()
            || path == "."
            || path.contains(['*', '?'])
            || path.starts_with('/')
            || path.split('/').any(|p| p == "..")
        {
            return Err(format!(
                "Scope requires explicit repository-relative paths, got {path:?}"
            ));
        }
    }
    allowed.sort();
    let before = allowed.len();
    allowed.dedup();
    if before != allowed.len() {
        return Err("Duplicate allowed scope paths".into());
    }
    let mut actual = staged.to_vec();
    actual.sort();
    if allowed != actual {
        return Err("Scope paths must exactly match the staged paths; missing, unrelated or unused paths are not allowed".into());
    }
    Ok(())
}

fn forbidden(path: &str, pattern: &str) -> bool {
    let expression = format!("^{}$", regex::escape(pattern).replace("\\*", ".*"));
    let Ok(pattern) = regex::Regex::new(&expression) else {
        return false;
    };
    pattern.is_match(path) || path.split('/').any(|part| pattern.is_match(part))
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SourceInventory {
    pub files: Vec<(String, String)>,
    /// Paths excluded by the declared source corpus or explicit policy prefixes.
    pub excluded_paths: Vec<String>,
    pub coverage_errors: Vec<String>,
    pub source_warnings: Vec<String>,
}

/// Read tracked source text from staged index blobs or current working files.
/// Bounded source failures are errors,
/// Known generated/dependency directories and non-source assets are outside this corpus.
pub fn source_files(base: &Path, staged_only: bool) -> Result<Vec<(String, String)>, String> {
    let inventory = source_files_with_exclusions(base, staged_only, &[])?;
    if !inventory.coverage_errors.is_empty() {
        return Err(inventory.coverage_errors.join("; "));
    }
    Ok(inventory.files)
}

/// Exclusions are literal relative files or directory prefixes, never globs.
pub fn source_files_with_exclusions(
    base: &Path,
    staged_only: bool,
    prefixes: &[String],
) -> Result<SourceInventory, String> {
    for prefix in prefixes {
        if prefix.is_empty()
            || prefix == "."
            || prefix.starts_with('/')
            || prefix.contains(['*', '?', '\\', ':'])
            || prefix
                .split('/')
                .any(|p| p == ".." || p == "." || p.is_empty())
        {
            return Err(format!(
                "Source exclusion must be a literal relative file or directory: {prefix:?}"
            ));
        }
    }
    git::source_files(base, staged_only, prefixes)
}

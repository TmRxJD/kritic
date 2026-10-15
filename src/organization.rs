//! Repository-defined organization checks. Heuristics expose review gaps rather than
//! claiming that directory names or comment presence prove good architecture.
mod imports;

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OrganizationPolicy {
    pub max_file_lines: usize,
    pub hard_file_lines: usize,
    pub max_folder_files: usize,
    pub max_depth: usize,
    pub long_function_lines: usize,
    pub complex_function_branches: usize,
    pub naming_rules: Vec<NamingRule>,
    /// Directory basename rules; extensions must be empty.
    pub directory_naming_rules: Vec<NamingRule>,
    pub layers: Vec<Layer>,
    /// Explicit namespace prefixes (e.g. `@/`) mapped to repository-relative roots.
    pub import_aliases: BTreeMap<String, String>,
    pub forbidden_prefixes: Vec<String>,
    pub required_prefixes: Vec<String>,
    pub baselines: BTreeMap<String, LineBaseline>,
}

impl Default for OrganizationPolicy {
    fn default() -> Self {
        Self {
            max_file_lines: 500,
            hard_file_lines: 1000,
            max_folder_files: 40,
            max_depth: 8,
            long_function_lines: 60,
            complex_function_branches: 8,
            naming_rules: vec![],
            directory_naming_rules: vec![],
            layers: vec![],
            import_aliases: BTreeMap::new(),
            forbidden_prefixes: vec![],
            required_prefixes: vec![],
            baselines: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamingRule {
    pub prefix: String,
    /// Extensions without a dot; empty applies to every file under the prefix.
    pub extensions: Vec<String>,
    /// Regex applied to the complete file name, including its extension.
    pub pattern: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub name: String,
    pub prefix: String,
    pub purpose: String,
    /// Other layer names this layer may import. Same-layer imports are allowed.
    pub allow_imports_from: Vec<String>,
    /// Package names or exact subpath prefixes forbidden in this layer.
    #[serde(default)]
    pub forbidden_imports: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineBaseline {
    pub max_lines: usize,
    pub justification: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Finding {
    pub code: String,
    pub path: String,
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FileSize {
    pub path: String,
    pub physical_lines: usize,
    pub nonblank_lines: usize,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct ImportCoverage {
    pub alias_imports: usize,
    pub external_imports: usize,
    pub unparsed_import_lines: usize,
    pub relative_imports: usize,
    pub resolved_imports: usize,
    pub unresolved_imports: usize,
    pub unclassified_edges: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct OrganizationReport {
    pub policy_valid: bool,
    pub compliant: bool,
    /// Deterministic policy compliance only; never architectural quality approval.
    pub approval: bool,
    pub policy_errors: Vec<String>,
    pub violations: Vec<Finding>,
    pub recommendations: Vec<Finding>,
    pub files: Vec<FileSize>,
    pub imports: ImportCoverage,
    pub coverage_notes: Vec<String>,
}

pub(super) fn finding(
    code: &str,
    path: &str,
    line: Option<usize>,
    message: impl Into<String>,
) -> Finding {
    Finding {
        code: code.into(),
        path: path.into(),
        line,
        message: message.into(),
    }
}

pub(super) fn valid_path(path: &str, allow_empty: bool) -> bool {
    (allow_empty && path.is_empty())
        || (!path.is_empty()
            && !path.contains(['\\', ':'])
            && path
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != ".."))
}

pub(super) fn under(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|s| s.starts_with('/'))
}

fn validation_findings(policy: &OrganizationPolicy) -> Vec<Finding> {
    let mut errors = vec![];
    let mut invalid = |message: String| errors.push(finding("invalid_policy", "", None, message));
    if let Err(error) = imports::validate_aliases(&policy.import_aliases) {
        invalid(error);
    }
    if policy.max_file_lines == 0
        || policy.hard_file_lines < policy.max_file_lines
        || policy.max_folder_files == 0
        || policy.max_depth == 0
        || policy.long_function_lines == 0
        || policy.complex_function_branches == 0
    {
        invalid("Thresholds must be positive and hard_file_lines must be >= max_file_lines".into());
    }
    if policy.naming_rules.len() + policy.directory_naming_rules.len() > 100
        || policy.layers.len() > 100
        || policy.baselines.len() > 10000
    {
        invalid("Policy exceeds supported rule/baseline limits".into());
        return errors;
    }
    for rule in policy
        .naming_rules
        .iter()
        .chain(&policy.directory_naming_rules)
    {
        if !valid_path(&rule.prefix, true)
            || rule.purpose.trim().is_empty()
            || rule.pattern.len() > 1024
            || Regex::new(&rule.pattern).is_err()
            || rule
                .extensions
                .iter()
                .any(|e| e.is_empty() || !e.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            invalid(format!(
                "Invalid naming rule for {:?}; require a valid regex, path, extensions and purpose",
                rule.prefix
            ));
        }
    }
    if policy
        .directory_naming_rules
        .iter()
        .any(|rule| !rule.extensions.is_empty())
    {
        invalid("Directory naming rules must have empty extensions".into());
    }
    let names: BTreeSet<_> = policy.layers.iter().map(|l| l.name.as_str()).collect();
    let prefixes: BTreeSet<_> = policy.layers.iter().map(|l| l.prefix.as_str()).collect();
    if names.len() != policy.layers.len() || prefixes.len() != policy.layers.len() {
        invalid("Layer names and prefixes must be unique".into());
    }
    for layer in &policy.layers {
        if layer.name.trim().is_empty()
            || layer.purpose.trim().is_empty()
            || !valid_path(&layer.prefix, true)
            || layer.forbidden_imports.len() > 100
            || layer.forbidden_imports.iter().any(|specifier| {
                specifier.is_empty()
                    || specifier.len() > 256
                    || specifier.starts_with(['.', '/'])
                    || specifier.ends_with('/')
                    || specifier.contains("//")
                    || !specifier
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "@/_.:-".contains(c))
            })
            || layer
                .allow_imports_from
                .iter()
                .any(|name| !names.contains(name.as_str()))
        {
            invalid(format!(
                "Invalid layer {:?}; require name, purpose, prefix and known dependency names",
                layer.name
            ));
        }
    }
    for prefix in policy
        .forbidden_prefixes
        .iter()
        .chain(&policy.required_prefixes)
    {
        if !valid_path(prefix, false) {
            invalid(format!("Invalid organization prefix {prefix:?}"));
        }
    }
    for (path, baseline) in &policy.baselines {
        if !valid_path(path, false)
            || baseline.max_lines <= policy.max_file_lines
            || baseline.justification.trim().len() < 20
        {
            invalid(format!(
                "Invalid baseline {path:?}; require an oversized-file ceiling and meaningful justification (>=20 characters)"
            ));
        }
    }
    errors
}

pub fn validate(policy: &OrganizationPolicy) -> Result<(), String> {
    let findings = validation_findings(policy);
    if findings.is_empty() {
        Ok(())
    } else {
        Err(findings
            .into_iter()
            .map(|f| f.message)
            .collect::<Vec<_>>()
            .join("; "))
    }
}

/// Audit an explicit, complete source inventory. Partial inventories cannot establish
/// folder counts, baseline freshness or import resolution for an entire repository.
pub fn audit(files: &[(String, String)], policy: &OrganizationPolicy) -> OrganizationReport {
    let errors = validation_findings(policy);
    let mut report = OrganizationReport {
        policy_valid: errors.is_empty(), compliant: false, approval: false,
        policy_errors: errors.iter().map(|f| f.message.clone()).collect(), violations: errors,
        recommendations: vec![], files: vec![], imports: ImportCoverage::default(),
        coverage_notes: vec![
            "Only supplied files are audited; callers must declare inventory exclusions and completeness.".into(),
            "Line counts include comments; nonblank counts are not language-aware executable LoC.".into(),
            "Function/comment and import parsing are heuristics, not semantic architecture or documentation proofs; unconfigured aliases, computed or dynamic imports, Rust/Python module syntax and external package resolution remain unverified. Configured aliases resolve only within the supplied inventory; recognized static package specifiers are checked against explicit forbidden_imports rules.".into(),
            "Comment presence does not establish sufficient explanation, and naming/purpose declarations do not prove cohesive responsibilities.".into(),
        ],
    };
    if !report.policy_valid {
        return report;
    }
    for (prefix, purpose) in policy.layers.iter().map(|l| (&l.prefix, &l.purpose)).chain(
        policy
            .naming_rules
            .iter()
            .chain(&policy.directory_naming_rules)
            .map(|r| (&r.prefix, &r.purpose)),
    ) {
        if purpose.split_whitespace().count() < 3 {
            report.recommendations.push(finding("review_purpose_declaration", prefix, None,
                "Very short purpose declaration: describe owned responsibilities and boundaries. Word count is only a review trigger; longer declarations may also be vague"));
        }
    }
    if files.len() > 20000
        || files.iter().any(|(_, text)| text.len() > 4 * 1024 * 1024)
        || files.iter().map(|(_, text)| text.len()).sum::<usize>() > 128 * 1024 * 1024
    {
        report.violations.push(finding(
            "inventory_limit",
            "",
            None,
            "Inventory exceeds 20,000 files, 4 MiB/file or 128 MiB total; audit not performed",
        ));
        return report;
    }
    let paths: BTreeSet<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
    if paths.len() != files.len() || paths.iter().any(|path| !valid_path(path, false)) {
        report.violations.push(finding(
            "invalid_inventory",
            "",
            None,
            "Paths must be unique normalized repository-relative paths",
        ));
        return report;
    }
    let mut folders: BTreeMap<&str, usize> = BTreeMap::new();
    for (path, text) in files {
        let physical = text.lines().count();
        report.files.push(FileSize {
            path: path.clone(),
            physical_lines: physical,
            nonblank_lines: text.lines().filter(|line| !line.trim().is_empty()).count(),
        });
        *folders
            .entry(path.rsplit_once('/').map_or("", |(parent, _)| parent))
            .or_default() += 1;
        let baseline = policy.baselines.get(path);
        if let Some(baseline) = baseline {
            if physical > baseline.max_lines {
                report.violations.push(finding(
                    "baseline_growth",
                    path,
                    None,
                    format!(
                        "{physical} lines exceed justified no-growth ceiling {}",
                        baseline.max_lines
                    ),
                ));
            } else if physical <= policy.max_file_lines {
                report.violations.push(finding(
                    "stale_baseline",
                    path,
                    None,
                    "File no longer needs a line baseline; remove it",
                ));
            } else if physical < baseline.max_lines {
                report.recommendations.push(finding(
                    "tighten_baseline",
                    path,
                    None,
                    format!(
                        "Reduce baseline ceiling from {} to {physical} after this shrink",
                        baseline.max_lines
                    ),
                ));
            }
        } else if physical > policy.hard_file_lines {
            report.violations.push(finding("file_line_limit", path, None, format!("{physical} physical lines exceed hard ceiling {}; split cohesive responsibilities or record an explicit justified no-growth baseline", policy.hard_file_lines)));
        }
        if physical > policy.max_file_lines {
            report.recommendations.push(finding("refactor_file", path, None, format!("{physical} physical lines exceed review threshold {}; examine cohesive responsibilities and extract modules around purpose, not arbitrary line counts", policy.max_file_lines)));
        }
        if path.split('/').count().saturating_sub(1) > policy.max_depth {
            report.violations.push(finding(
                "path_depth",
                path,
                None,
                format!("Folder depth exceeds {}", policy.max_depth),
            ));
        }
        for prefix in &policy.forbidden_prefixes {
            if under(path, prefix) {
                report.violations.push(finding(
                    "forbidden_location",
                    path,
                    None,
                    format!("Files are forbidden under {prefix}"),
                ));
            }
        }
        if !policy.required_prefixes.is_empty()
            && !policy.required_prefixes.iter().any(|p| under(path, p))
        {
            report.violations.push(finding(
                "misplaced_file",
                path,
                None,
                "File is outside all declared source roots",
            ));
        }
        let leaf = path.rsplit('/').next().unwrap_or(path);
        let extension = leaf.rsplit_once('.').map_or("", |(_, ext)| ext);
        for rule in &policy.naming_rules {
            if under(path, &rule.prefix)
                && (rule.extensions.is_empty() || rule.extensions.iter().any(|e| e == extension))
                && !Regex::new(&rule.pattern)
                    .expect("validated regex")
                    .is_match(leaf)
            {
                report.violations.push(finding(
                    "file_naming",
                    path,
                    None,
                    format!(
                        "Name violates {} for purpose: {}",
                        rule.pattern, rule.purpose
                    ),
                ));
            }
        }
        document_functions(path, text, policy, &mut report);
    }
    for (folder, count) in folders {
        if count > policy.max_folder_files {
            report.recommendations.push(finding("folder_density", folder, None, format!("{count} direct files exceed review threshold {}; consider purpose-based grouping following repository terminology", policy.max_folder_files)));
        }
    }
    let mut directories = BTreeSet::new();
    for path in &paths {
        let mut remainder = *path;
        while let Some((parent, _)) = remainder.rsplit_once('/') {
            directories.insert(parent);
            remainder = parent;
        }
    }
    for directory in directories {
        for rule in &policy.directory_naming_rules {
            if under(directory, &rule.prefix)
                && !Regex::new(&rule.pattern)
                    .expect("validated regex")
                    .is_match(directory.rsplit('/').next().unwrap_or(directory))
            {
                report.violations.push(finding(
                    "folder_naming",
                    directory,
                    None,
                    format!(
                        "Directory name violates {} for purpose: {}",
                        rule.pattern, rule.purpose
                    ),
                ));
            }
        }
    }
    for path in policy.baselines.keys() {
        if !paths.contains(path.as_str()) {
            report.recommendations.push(finding("unverified_baseline_file", path, None, "Baseline file is absent from this inventory; its existence and freshness are unverified"));
        }
    }
    if policy.layers.is_empty() {
        report.recommendations.push(finding("undefined_layers", "", None, "Declare repository-specific layer prefixes, purposes and allowed dependency directions to check organization boundaries"));
    }
    imports::audit(files, policy, &paths, &mut report);
    report.compliant = report.violations.is_empty();
    report.approval = report.compliant;
    report
}

fn document_functions(
    path: &str,
    text: &str,
    policy: &OrganizationPolicy,
    report: &mut OrganizationReport,
) {
    let lines: Vec<_> = text.lines().collect();
    for function in crate::metrics::functions(path, text) {
        if function.lines <= policy.long_function_lines
            && function.complexity.saturating_sub(1) < policy.complex_function_branches
        {
            continue;
        }
        let previous = lines[..function.start_line.saturating_sub(1)]
            .iter()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim());
        let has_comment = previous.is_some_and(|line| {
            line.starts_with("//")
                || (line.starts_with('#') && path.ends_with(".py"))
                || line.starts_with("/*")
                || line.starts_with('*')
                || line.ends_with("*/")
                || line.ends_with("\"\"\"")
                || line.ends_with("'''")
        });
        report.recommendations.push(finding(if has_comment { "review_function_explanation" } else { "missing_function_explanation" }, path, Some(function.start_line),
            format!("{} spans {} lines with ~{} branches. {} Review responsibility, invariants, rationale and edge cases; metrics cannot determine comment sufficiency", function.name, function.lines, function.complexity.saturating_sub(1),
                if has_comment { "Adjacent comment detected, contents remain unverified." } else { "No adjacent explanatory comment detected (heuristic)." })));
    }
}

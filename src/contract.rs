//! Work contracts: fresh checks, explicit evidence and separately audited enforcement wiring.
//! Prose and model opinions cannot turn an uncovered requirement into a verified one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::checks::{CommandResult, Status, run_command};
use crate::config::Config;

const MAX_FILE: u64 = 1024 * 1024;
fn yes() -> bool {
    true
}
fn hour() -> u64 {
    3600
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub version: u32,
    pub title: String,
    #[serde(default)]
    pub sources: Vec<String>,
    pub requirements: Vec<Requirement>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub id: String,
    pub description: String,
    #[serde(default = "yes")]
    pub required: bool,
    #[serde(default)]
    pub require_enforcement: bool,
    #[serde(default)]
    pub checks: Vec<ContractCheck>,
    #[serde(default)]
    pub enforcement: Vec<Binding>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub path: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContractCheck {
    FileExists {
        path: String,
    },
    Content {
        path: String,
        pattern: String,
        #[serde(default = "yes")]
        present: bool,
    },
    Command {
        command: String,
    },
    /// External report is accepted only against current input hashes and a bounded age.
    Report {
        path: String,
        pointer: String,
        expected: Value,
        inputs: BTreeMap<String, String>,
        #[serde(default = "hour")]
        max_age_s: u64,
    },
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Verified,
    Violated,
    Unverified,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Evidence {
    pub kind: String,
    pub outcome: Outcome,
    pub detail: String,
    pub artifacts: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandResult>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RequirementResult {
    pub id: String,
    pub description: String,
    pub required: bool,
    pub require_enforcement: bool,
    pub outcome: Outcome,
    /// configured = declared textual wiring is present; it does not attest a CI run occurred.
    pub enforcement: String,
    pub checks: Vec<Evidence>,
    pub wiring: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Source {
    pub path: String,
    pub sha256: Option<String>,
    pub status: String,
    /// Retained for the calling agent to interpret; not treated as executable requirements.
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ContractResult {
    pub tool: String,
    pub advisory: bool,
    pub run_id: String,
    pub checked_at: String,
    pub contract_sha256: Option<String>,
    pub title: Option<String>,
    pub approval: Option<bool>,
    pub coverage: f64,
    pub requirements: Vec<RequirementResult>,
    pub sources: Vec<Source>,
    pub task: String,
    pub reasons: Vec<String>,
    pub limitations: Vec<String>,
    pub full_result: Option<String>,
}

pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Every file read must stay in the selected repository, including symlink resolution.
fn resolve(base: &Path, path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if path.components().any(|c| c.as_os_str() == ".git") {
        return Err("Git internals are not contract evidence".into());
    }
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let full = full
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !full.starts_with(base) {
        return Err("evidence path escapes the repository".into());
    }
    if full
        .strip_prefix(base)
        .map_err(|_| "evidence path escapes repository")?
        .components()
        .any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(".git"))
    {
        return Err("Git internals are not contract evidence".into());
    }
    if !full.is_file() {
        return Err("evidence path is not a file".into());
    }
    Ok(full)
}

fn read(base: &Path, path: &str) -> Result<Vec<u8>, String> {
    let full = resolve(base, path)?;
    let size = full.metadata().map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err("contract evidence exceeds 1 MiB".into());
    }
    let bytes = std::fs::read(full).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("contract evidence exceeds 1 MiB".into());
    }
    Ok(bytes)
}

fn text_check(
    base: &Path,
    path: &str,
    pattern: &str,
    present: bool,
) -> Result<(bool, String), String> {
    if pattern.trim().is_empty() {
        return Err("empty patterns cannot prove compliance".into());
    }
    let re = Regex::new(pattern).map_err(|e| format!("invalid evidence regex: {e}"))?;
    if re.is_match("") {
        return Err("vacuous pattern matches empty input".into());
    }
    let bytes = read(base, path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "evidence is not UTF-8 text")?;
    Ok((re.is_match(text) == present, hash(&bytes)))
}

fn evidence(kind: &str, outcome: Outcome, detail: impl Into<String>) -> Evidence {
    Evidence {
        kind: kind.into(),
        outcome,
        detail: detail.into(),
        artifacts: BTreeMap::new(),
        command: None,
    }
}

async fn evaluate(
    check: &ContractCheck,
    base: &Path,
    cfg: &Config,
    run_commands: bool,
) -> Evidence {
    match check {
        ContractCheck::FileExists { path } => match read(base, path) {
            Ok(bytes) => {
                let mut e = evidence(
                    "file_exists",
                    Outcome::Verified,
                    format!("file exists: {path}"),
                );
                e.artifacts.insert(path.clone(), hash(&bytes));
                e
            }
            Err(e) => evidence("file_exists", Outcome::Violated, e),
        },
        ContractCheck::Content {
            path,
            pattern,
            present,
        } => match text_check(base, path, pattern, *present) {
            Ok((ok, hash)) => {
                let mut e = evidence(
                    "content",
                    if ok {
                        Outcome::Verified
                    } else {
                        Outcome::Violated
                    },
                    format!(
                        "declared text assertion {}: {path}",
                        if ok { "holds" } else { "fails" }
                    ),
                );
                e.artifacts.insert(path.clone(), hash);
                e
            }
            Err(e) => evidence("content", Outcome::Unverified, e),
        },
        ContractCheck::Command { command } => {
            if command.trim().is_empty() {
                return evidence("command", Outcome::Unverified, "empty command");
            }
            if !run_commands {
                return evidence(
                    "command",
                    Outcome::Unverified,
                    "command not run; explicitly pass run_commands=true after reviewing the contract",
                );
            }
            let result = run_command(command, base, cfg.command_timeout_s).await;
            let mut e = evidence(
                "command",
                if result.status == Status::Pass {
                    Outcome::Verified
                } else {
                    Outcome::Violated
                },
                "fresh command exit status; does not establish test quality or coverage",
            );
            e.command = Some(result);
            e
        }
        ContractCheck::Report {
            path,
            pointer,
            expected,
            inputs,
            max_age_s,
        } => {
            let verify = || -> Result<Evidence, String> {
                if inputs.is_empty() || *max_age_s == 0 {
                    return Err(
                        "reports require nonempty input hashes and a positive max_age_s".into(),
                    );
                }
                let full = resolve(base, path)?;
                let age = SystemTime::now()
                    .duration_since(
                        full.metadata()
                            .map_err(|e| e.to_string())?
                            .modified()
                            .map_err(|e| e.to_string())?,
                    )
                    .map_err(|_| "report timestamp is in the future")?;
                if age > Duration::from_secs(*max_age_s) {
                    return Err("stale report; rerun the check".into());
                }
                let bytes = read(base, path)?;
                let report: Value =
                    serde_json::from_slice(&bytes).map_err(|_| "report is not JSON")?;
                let mut e = evidence(
                    "report",
                    Outcome::Unverified,
                    "input hashes verified; report contents are supplied evidence, not authenticated execution",
                );
                e.artifacts.insert(path.clone(), hash(&bytes));
                for (input, expected_hash) in inputs {
                    let actual = hash(&read(base, input)?);
                    if &actual != expected_hash {
                        return Err(format!("stale evidence: input hash changed for {input}"));
                    }
                    e.artifacts.insert(input.clone(), actual);
                }
                let observed = report
                    .pointer(pointer)
                    .ok_or("report pointer missing or invalid")?;
                e.outcome = if observed == expected {
                    Outcome::Verified
                } else {
                    Outcome::Violated
                };
                Ok(e)
            };
            verify().unwrap_or_else(|e| evidence("report", Outcome::Unverified, e))
        }
    }
}

pub fn parse(text: &str) -> Result<Contract, String> {
    let contract: Contract =
        toml::from_str(text).map_err(|e| format!("invalid work contract: {e}"))?;
    if contract.version != 1 || contract.title.trim().is_empty() || contract.requirements.is_empty()
    {
        return Err("contract requires version=1, a title and at least one requirement".into());
    }
    if contract.requirements.len() > 100 || contract.sources.len() > 16 {
        return Err("contract exceeds 100 requirements or 16 sources".into());
    }
    let mut ids = BTreeSet::new();
    for r in &contract.requirements {
        if r.id.trim().is_empty() || r.description.trim().is_empty() || !ids.insert(&r.id) {
            return Err("requirement IDs must be nonempty/unique and descriptions nonempty".into());
        }
        if r.checks.len() > 20 || r.enforcement.len() > 20 {
            return Err("requirement exceeds 20 checks or enforcement bindings".into());
        }
        for c in &r.checks {
            if let ContractCheck::Report {
                inputs, max_age_s, ..
            } = c
                && (inputs.len() > 100 || *max_age_s > 86400 * 30)
            {
                return Err("report exceeds 100 inputs or 30 days maximum age".into());
            }
        }
    }
    Ok(contract)
}

pub async fn critique(
    cfg: &Config,
    base: &Path,
    path: Option<&str>,
    task: &str,
    run_commands: bool,
) -> ContractResult {
    let now = chrono::Utc::now();
    let mut result = ContractResult {
        tool: "contract_critic".into(), advisory: true, run_id: format!("contract-{}", now.timestamp_nanos_opt().unwrap_or_default()),
        checked_at: now.to_rfc3339(), contract_sha256: None, title: None, approval: None, coverage: 0.0,
        requirements: Vec::new(), sources: Vec::new(), task: task.into(), reasons: Vec::new(), full_result: None,
        limitations: vec!["Verification covers only the declared assertions; passing a weak check cannot prove the full prose requirement.".into(),
            "Enforcement bindings audit declared text in CI/hooks/tests; they do not prove that CI ran or branch protection is enabled.".into(),
            "Imported reports are not signed execution attestations. Freshness is checked using file age and current input hashes.".into(),
            "AGENTS.md and task prose require interpretation by the calling agent; they are never automatically marked satisfied.".into()],
    };
    let base = match base.canonicalize() {
        Ok(b) if b.is_dir() => b,
        _ => {
            result
                .reasons
                .push("base_dir is not an accessible directory".into());
            return result;
        }
    };
    let path = path.unwrap_or(".kritic/contract.toml");
    let loaded = read(&base, path).and_then(|bytes| {
        result.contract_sha256 = Some(hash(&bytes));
        let text = std::str::from_utf8(&bytes).map_err(|_| "contract is not UTF-8")?;
        parse(text)
    });
    let mut sources = vec!["AGENTS.md".to_string()];
    match loaded {
        Err(e) => result.reasons.push(e),
        Ok(contract) => {
            result.title = Some(contract.title);
            sources.extend(contract.sources);
            for r in contract.requirements {
                let mut checks = Vec::new();
                for check in &r.checks {
                    checks.push(evaluate(check, &base, cfg, run_commands).await);
                }
                let mut wiring = Vec::new();
                for b in &r.enforcement {
                    let mut e = match text_check(&base, &b.path, &b.pattern, true) {
                        Ok((ok, sha)) => {
                            let mut e = evidence(
                                "enforcement_binding",
                                if ok {
                                    Outcome::Verified
                                } else {
                                    Outcome::Violated
                                },
                                format!(
                                    "declared wiring {}: {}",
                                    if ok { "present" } else { "missing" },
                                    b.path
                                ),
                            );
                            e.artifacts.insert(b.path.clone(), sha);
                            e
                        }
                        Err(e) => evidence("enforcement_binding", Outcome::Unverified, e),
                    };
                    e.detail.push_str(" (static inspection)");
                    wiring.push(e);
                }
                let enforcement = if wiring.is_empty()
                    || wiring.iter().any(|e| e.outcome == Outcome::Unverified)
                {
                    "unverified"
                } else if wiring.iter().any(|e| e.outcome == Outcome::Violated) {
                    "missing"
                } else {
                    "configured"
                };
                let outcome = if checks.iter().any(|e| e.outcome == Outcome::Violated)
                    || (r.require_enforcement && enforcement == "missing")
                {
                    Outcome::Violated
                } else if checks.is_empty()
                    || checks.iter().any(|e| e.outcome == Outcome::Unverified)
                    || (r.require_enforcement && enforcement == "unverified")
                {
                    Outcome::Unverified
                } else {
                    Outcome::Verified
                };
                result.requirements.push(RequirementResult {
                    id: r.id,
                    description: r.description,
                    required: r.required,
                    require_enforcement: r.require_enforcement,
                    outcome,
                    enforcement: enforcement.into(),
                    checks,
                    wiring,
                });
            }
            // Commands later in the run may mutate artifacts checked earlier. Revalidate all
            // captured hashes before claiming coverage, including the enforcement bindings.
            for r in &mut result.requirements {
                for e in r.checks.iter_mut().chain(r.wiring.iter_mut()) {
                    if e.artifacts.iter().any(|(path, sha)| {
                        read(&base, path)
                            .map(|bytes| hash(&bytes) != *sha)
                            .unwrap_or(true)
                    }) {
                        e.outcome = Outcome::Unverified;
                        e.detail =
                            "evidence changed during the contract run; rerun against stable inputs"
                                .into();
                    }
                }
                r.enforcement = if r.wiring.is_empty()
                    || r.wiring.iter().any(|e| e.outcome == Outcome::Unverified)
                {
                    "unverified"
                } else if r.wiring.iter().any(|e| e.outcome == Outcome::Violated) {
                    "missing"
                } else {
                    "configured"
                }
                .into();
                r.outcome = if r.checks.iter().any(|e| e.outcome == Outcome::Violated)
                    || (r.require_enforcement && r.enforcement == "missing")
                {
                    Outcome::Violated
                } else if r.checks.is_empty()
                    || r.checks.iter().any(|e| e.outcome == Outcome::Unverified)
                    || (r.require_enforcement && r.enforcement == "unverified")
                {
                    Outcome::Unverified
                } else {
                    Outcome::Verified
                };
            }
            let required: Vec<_> = result.requirements.iter().filter(|r| r.required).collect();
            let verified = required
                .iter()
                .filter(|r| r.outcome == Outcome::Verified)
                .count();
            if !required.is_empty() {
                result.coverage = verified as f64 / required.len() as f64;
                result.approval = if required.iter().any(|r| r.outcome == Outcome::Violated) {
                    Some(false)
                } else if verified == required.len() {
                    Some(true)
                } else {
                    None
                };
            } else {
                result
                    .reasons
                    .push("no required acceptance criteria; approval is unverified".into());
            }
            result.reasons.extend(
                required
                    .iter()
                    .filter(|r| r.outcome != Outcome::Verified)
                    .map(|r| format!("{}: {:?}", r.id, r.outcome)),
            );
        }
    }
    if let Some(before) = &result.contract_sha256
        && read(&base, path)
            .map(|bytes| hash(&bytes) != *before)
            .unwrap_or(true)
    {
        if result.approval != Some(false) {
            result.approval = None;
        }
        result
            .reasons
            .push("contract changed during evaluation; acceptance scope is unverified".into());
    }
    let mut seen = BTreeSet::new();
    for path in sources {
        if !seen.insert(path.clone()) {
            continue;
        }
        let source = match read(&base, &path) {
            Ok(bytes) => Source {
                path,
                sha256: Some(hash(&bytes)),
                status: "prose_unverified".into(),
                text: Some(crate::checks::clip(&String::from_utf8_lossy(&bytes), 8000)),
            },
            Err(e) => Source {
                path,
                sha256: None,
                status: e,
                text: None,
            },
        };
        result.sources.push(source);
    }
    result.full_result = crate::tools::save_full(cfg, "contract", &result);
    result
}

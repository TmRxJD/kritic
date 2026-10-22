//! Phase 3 critics. Real tools produce every finding and metric; Kev only triages findings and
//! scores metrics. Approval rules are deterministic and stated in each result.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};

use crate::config::Config;
use crate::kev::Decider;
use crate::metrics::{Duplicate, FunctionMetric, duplicates, functions};
use crate::scan::{self, Finding, Runner, ToolRun, severity_rank};

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn context(base: &Path, f: &Finding, radius: usize) -> String {
    let Ok(text) = std::fs::read_to_string(base.join(&f.file)) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    if f.line == 0 || lines.is_empty() {
        return String::new();
    }
    let i = (f.line as usize).saturating_sub(1).min(lines.len() - 1);
    let lo = i.saturating_sub(radius);
    let hi = (i + radius + 1).min(lines.len());
    lines[lo..hi]
        .iter()
        .enumerate()
        .map(|(k, l)| format!("{:>4} {}", lo + k + 1, l))
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(1100)
        .collect()
}

// ================================================================== security

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TriagedFinding {
    #[serde(flatten)]
    pub finding: Finding,
    /// Kev: probability this is a real, exploitable issue in context. Advisory ordering only;
    /// it never removes a finding from the approval rule.
    pub p_true_positive: Option<f64>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct SecurityResult {
    pub tool: String,
    pub advisory: bool,
    pub base_dir: String,
    pub tools: Vec<ToolRun>,
    /// Sorted by severity, then Kev's p_true_positive.
    pub findings: Vec<TriagedFinding>,
    pub counts: BTreeMap<String, usize>,
    /// 0-10, deterministic from counts: 10 - 3*critical - 2*high - 0.5*medium (floor 0).
    pub security_score: Option<f64>,
    pub worst_severity: String,
    pub kev_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kev_error: Option<String>,
    pub findings_triaged: usize,
    pub approval: Option<bool>,
    pub reasons: Vec<String>,
    pub duration_ms: u64,
    /// Findings beyond the first 25 are only in this file.
    pub full_result: Option<String>,
}

pub fn security_score(counts: &BTreeMap<String, usize>) -> f64 {
    let c = |k: &str| *counts.get(k).unwrap_or(&0) as f64;
    round2((10.0 - 3.0 * c("critical") - 2.0 * c("high") - 0.5 * c("medium")).max(0.0))
}

pub async fn security<D: Decider>(
    d: &D,
    cfg: &Config,
    base: &Path,
    triage: bool,
    semgrep_config: &str,
) -> SecurityResult {
    let t0 = Instant::now();
    let mut r = SecurityResult {
        tool: "security_critic".into(),
        advisory: true,
        base_dir: base.display().to_string(),
        ..Default::default()
    };
    let proj = scan::detect(base);
    let bin = &cfg.tools_bin;
    let mut run = Runner::new(base, bin, cfg.scan_timeout_s);

    let gl_dir = crate::vision::run_dir(cfg, "gitleaks");
    let gl_cfg = std::fs::create_dir_all(&gl_dir).and_then(|_| scan::gitleaks_config(&gl_dir));
    match gl_cfg {
        Ok(conf) => {
            let conf = conf.display().to_string();
            let args = [
                "dir",
                ".",
                "--config",
                &conf,
                "--report-format",
                "json",
                "--report-path",
                "-",
                "--no-banner",
                "--exit-code",
                "0",
                "--log-level",
                "error",
            ];
            run.run("gitleaks", scan::tool_path(bin, "gitleaks"), &args, |o| {
                scan::parse_gitleaks(base, o)
            })
            .await;
        }
        Err(e) => run.skip("gitleaks", &format!("cannot write config: {e}")),
    }
    if proj.lockfiles {
        run.run(
            "osv-scanner",
            scan::tool_path(bin, "osv-scanner"),
            &["scan", "source", "-r", "--format", "json", "."],
            |o| scan::parse_osv(base, o),
        )
        .await;
    } else {
        run.skip("osv-scanner", "no dependency lockfile/manifest found");
    }
    if proj.python {
        let excludes = scan::bandit_excludes();
        run.run(
            "bandit",
            scan::tool_path(bin, "bandit"),
            &["-r", ".", "-f", "json", "-q", "-x", &excludes],
            |o| scan::parse_bandit(base, o),
        )
        .await;
    } else {
        run.skip("bandit", "no Python code");
    }
    run.run(
        "semgrep",
        scan::tool_path(bin, "semgrep"),
        &[
            "scan",
            "--config",
            semgrep_config,
            "--json",
            "--metrics=off",
            "--quiet",
            "--timeout",
            "30",
            ".",
        ],
        |o| scan::parse_semgrep(base, o),
    )
    .await;
    if proj.rust && base.join("Cargo.lock").exists() {
        let exe = scan::tool_path(&cfg.cargo_tools_bin, "cargo-audit");
        if exe.is_none() {
            // optional: osv-scanner already covers Cargo.lock, so a missing cross-check is not a failure
            run.skip(
                "cargo-audit",
                "not installed (osv-scanner covers Cargo.lock)",
            );
        } else {
            run.run(
                "cargo-audit",
                exe,
                &["audit", "--json"],
                scan::parse_cargo_audit,
            )
            .await;
        }
    }
    r.tools = run.runs;
    let mut findings = run.findings;
    findings.sort_by_key(|f| std::cmp::Reverse(severity_rank(&f.severity)));

    for f in &findings {
        *r.counts.entry(f.severity.clone()).or_default() += 1;
    }
    r.security_score = Some(security_score(&r.counts));
    r.worst_severity = findings
        .first()
        .map(|f| f.severity.clone())
        .unwrap_or_else(|| "none".into());

    // Kev triage of the most severe code findings (dependency advisories have no code context)
    let mut triaged: Vec<TriagedFinding> = findings
        .into_iter()
        .map(|finding| TriagedFinding {
            finding,
            p_true_positive: None,
        })
        .collect();
    if triage {
        let spec = crate::spec::get("security_triage");
        let q = spec.wire();
        let mut ok = true;
        for t in triaged
            .iter_mut()
            .filter(|t| t.finding.line > 0)
            .take(cfg.max_triage)
        {
            let ctx: String = context(base, &t.finding, 6).chars().take(1400).collect();
            let finding: String = format!(
                "[{}] {} {} in {}:{}: {}",
                t.finding.severity,
                t.finding.tool,
                t.finding.rule,
                t.finding.file,
                t.finding.line,
                t.finding.message
            )
            .chars()
            .take(600)
            .collect();
            let Ok(state) = spec.state(&[("finding", finding), ("code", ctx)]) else {
                continue;
            };
            match d.decide(&state, &q).await {
                Ok(dec) => {
                    t.p_true_positive = Some(round2(dec.answers["real"].value));
                    r.findings_triaged += 1;
                }
                Err(e) => {
                    ok = false;
                    r.kev_error = Some(e);
                    break;
                }
            }
        }
        r.kev_ok = Some(ok);
        triaged.sort_by(|a, b| {
            severity_rank(&b.finding.severity)
                .cmp(&severity_rank(&a.finding.severity))
                .then(
                    b.p_true_positive
                        .unwrap_or(1.0)
                        .total_cmp(&a.p_true_positive.unwrap_or(1.0)),
                )
        });
    }
    r.findings = triaged;

    for t in r.tools.iter().filter(|t| t.status == "failed") {
        r.reasons
            .push(format!("{} failed to run: {}", t.tool, t.detail));
    }
    let blocking: Vec<&TriagedFinding> = r
        .findings
        .iter()
        .filter(|t| severity_rank(&t.finding.severity) >= 3)
        .collect();
    for t in blocking.iter().take(15) {
        r.reasons.push(format!(
            "[{}] {} {} at {}:{}: {}{}",
            t.finding.severity,
            t.finding.tool,
            t.finding.rule,
            t.finding.file,
            t.finding.line,
            t.finding.message,
            t.p_true_positive
                .map(|p| format!(" (Kev p_true={p:.2})"))
                .unwrap_or_default()
        ));
    }
    if blocking.len() > 15 {
        r.reasons.push(format!(
            "... and {} more high/critical findings",
            blocking.len() - 15
        ));
    }
    let failed = r.tools.iter().any(|t| t.status == "failed");
    let covered = r.tools.iter().any(|t| t.status == "ran");
    if failed || !covered {
        r.security_score = None;
        r.reasons
            .push("security scan coverage is incomplete".into());
    }
    r.approval = if !blocking.is_empty() {
        Some(false)
    } else if failed || !covered {
        None
    } else {
        Some(true)
    };
    r.duration_ms = t0.elapsed().as_millis() as u64;
    r
}

// ================================================================== code quality

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ScoredFunction {
    #[serde(flatten)]
    pub metric: FunctionMetric,
    /// Kev: probability the function is hard to understand or maintain.
    pub p_hard_to_maintain: Option<f64>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct QualityResult {
    pub tool: String,
    pub advisory: bool,
    pub tools: Vec<ToolRun>,
    pub lint_findings: Vec<Finding>,
    pub lint_counts: BTreeMap<String, usize>,
    pub functions_analyzed: usize,
    /// Deterministic organization policy applied only to the explicitly supplied files.
    pub organization: Option<crate::organization::OrganizationReport>,
    /// Worst functions by complexity, with Kev's maintainability judgment.
    pub hotspots: Vec<ScoredFunction>,
    pub duplicates: Vec<Duplicate>,
    pub max_complexity: usize,
    pub max_function_lines: usize,
    /// 0-10: 10 * (1 - line-weighted mean p_hard_to_maintain over hotspots). Null without Kev.
    pub maintainability: Option<f64>,
    /// 0-10, deterministic: starts at 10, minus lint/complexity/length/duplication penalties.
    pub quality_score: Option<f64>,
    /// low | medium | high, deterministic from the same signals.
    pub risk_level: String,
    pub kev_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kev_error: Option<String>,
    pub approval: Option<bool>,
    pub reasons: Vec<String>,
    pub duration_ms: u64,
    pub full_result: Option<String>,
}

pub struct QualityOptions {
    pub max_complexity: usize,
    pub max_function_lines: usize,
    pub run_linters: bool,
    pub use_kev: bool,
}

pub async fn quality<D: Decider>(
    d: &D,
    cfg: &Config,
    base: &Path,
    files: &[(String, String)],
    opts: QualityOptions,
) -> QualityResult {
    let t0 = Instant::now();
    let mut r = QualityResult {
        tool: "code_quality_critic".into(),
        advisory: true,
        ..Default::default()
    };
    let organization_policy = match std::fs::symlink_metadata(base.join(".kritic/repository.toml"))
    {
        Ok(_) => crate::repository::load(base, false).map(|policy| policy.organization),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => Err(format!("read repository organization policy: {error}")),
    };
    let organization_failed = match organization_policy {
        Ok(policy) => {
            let report = crate::organization::audit(files, &policy);
            for violation in &report.violations {
                r.reasons.push(format!(
                    "organization {} at {}: {}",
                    violation.code, violation.path, violation.message
                ));
            }
            let failed = !report.approval;
            r.organization = Some(report);
            failed
        }
        Err(error) => {
            r.reasons
                .push(format!("organization policy failed: {error}"));
            true
        }
    };
    // 1. linters the project is set up for
    if opts.run_linters {
        let proj = scan::detect(base);
        let bin = &cfg.tools_bin;
        let mut run = Runner::new(base, bin, cfg.scan_timeout_s);
        if proj.python {
            let own_config = base.join("ruff.toml").exists()
                || base.join(".ruff.toml").exists()
                || base.join("pyproject.toml").exists();
            let mut args = vec![
                "check",
                "--output-format",
                "json",
                "--no-cache",
                "--exit-zero",
            ];
            if !own_config {
                args.extend([
                    "--select",
                    "E9,F,B,C90",
                    "--config",
                    "lint.mccabe.max-complexity=12",
                ]);
            }
            args.push(".");
            run.run("ruff", scan::tool_path(bin, "ruff"), &args, |o| {
                scan::parse_ruff(base, o)
            })
            .await;
        } else {
            run.skip("ruff", "no Python code");
        }
        if proj.rust {
            let cargo = scan::tool_path(Path::new("."), "cargo");
            run.run(
                "clippy",
                cargo,
                &[
                    "clippy",
                    "--all-targets",
                    "--message-format",
                    "json",
                    "--quiet",
                ],
                |o| Ok(scan::parse_clippy(base, o)),
            )
            .await;
        } else {
            run.skip("clippy", "no Cargo.toml");
        }
        let local_bin = base.join("node_modules").join(".bin");
        if proj.eslint {
            run.run(
                "eslint",
                scan::tool_path(
                    &local_bin,
                    if cfg!(windows) {
                        "eslint.cmd"
                    } else {
                        "eslint"
                    },
                )
                .or_else(|| scan::tool_path(&local_bin, "eslint")),
                &["-f", "json", "."],
                |o| scan::parse_eslint(base, o),
            )
            .await;
        } else {
            run.skip("eslint", "no eslint config");
        }
        if proj.tsconfig {
            run.run(
                "tsc",
                scan::tool_path(&local_bin, if cfg!(windows) { "tsc.cmd" } else { "tsc" })
                    .or_else(|| scan::tool_path(&local_bin, "tsc")),
                &["--noEmit", "--pretty", "false"],
                |o| Ok(scan::parse_tsc(base, o)),
            )
            .await;
        } else {
            run.skip("tsc", "no tsconfig.json");
        }
        r.tools = run.runs;
        r.lint_findings = run.findings;
        for f in &r.lint_findings {
            *r.lint_counts.entry(f.severity.clone()).or_default() += 1;
        }
    }
    // 2. metrics
    let mut fns: Vec<FunctionMetric> = files.iter().flat_map(|(f, t)| functions(f, t)).collect();
    r.functions_analyzed = fns.len();
    r.max_complexity = fns.iter().map(|f| f.complexity).max().unwrap_or(0);
    r.max_function_lines = fns.iter().map(|f| f.lines).max().unwrap_or(0);
    r.duplicates = duplicates(files, 8);
    let too_complex = fns
        .iter()
        .filter(|f| f.complexity > opts.max_complexity)
        .count();
    let too_long = fns
        .iter()
        .filter(|f| f.lines > opts.max_function_lines)
        .count();

    fns.sort_by(|a, b| b.complexity.cmp(&a.complexity).then(b.lines.cmp(&a.lines)));
    let mut hotspots: Vec<ScoredFunction> = fns
        .into_iter()
        .take(cfg.max_hotspots)
        .map(|metric| ScoredFunction {
            metric,
            p_hard_to_maintain: None,
        })
        .collect();
    // 3. Kev on hotspots
    if opts.use_kev && !hotspots.is_empty() {
        let spec = crate::spec::get("function_maintainability");
        let q = spec.wire();
        let mut ok = true;
        for h in hotspots.iter_mut() {
            let body: String = h
                .metric
                .body
                .chars()
                .take(cfg.chunk_chars.min(1400))
                .collect();
            let Ok(state) = spec.state(&[("function", body)]) else {
                continue;
            };
            match d.decide(&state, &q).await {
                Ok(dec) => h.p_hard_to_maintain = Some(round2(dec.answers["hard"].value)),
                Err(e) => {
                    ok = false;
                    r.kev_error = Some(e);
                    break;
                }
            }
        }
        r.kev_ok = Some(ok);
        // weight by length: a 60-line tangle matters more than a 2-line helper
        let (num, den) = hotspots
            .iter()
            .filter_map(|h| {
                h.p_hard_to_maintain
                    .map(|p| (p * h.metric.lines as f64, h.metric.lines as f64))
            })
            .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
        if ok && den > 0.0 {
            r.maintainability = Some(round2(10.0 * (1.0 - num / den)));
        }
    }
    r.hotspots = hotspots;
    // 4. deterministic score, risk, approval
    let lint_high = *r.lint_counts.get("high").unwrap_or(&0);
    let lint_med = *r.lint_counts.get("medium").unwrap_or(&0);
    let penalty = 1.5 * lint_high as f64
        + 0.3 * lint_med as f64
        + 1.0 * too_complex as f64
        + 0.5 * too_long as f64
        + 0.3 * r.duplicates.len() as f64;
    r.quality_score = Some(round2((10.0 - penalty).max(0.0)));
    r.risk_level = if lint_high > 0 || too_complex > 2 {
        "high"
    } else if lint_med > 3 || too_complex > 0 || too_long > 0 || r.duplicates.len() > 2 {
        "medium"
    } else {
        "low"
    }
    .into();
    for f in r
        .lint_findings
        .iter()
        .filter(|f| f.severity == "high")
        .take(15)
    {
        r.reasons.push(format!(
            "{} {} at {}:{}: {}",
            f.tool, f.rule, f.file, f.line, f.message
        ));
    }
    for h in r
        .hotspots
        .iter()
        .filter(|h| h.metric.complexity > opts.max_complexity)
    {
        r.reasons.push(format!(
            "{} in {}:{} has complexity {} > {}",
            h.metric.name,
            h.metric.file,
            h.metric.start_line,
            h.metric.complexity,
            opts.max_complexity
        ));
    }
    for h in r
        .hotspots
        .iter()
        .filter(|h| h.metric.lines > opts.max_function_lines)
    {
        r.reasons.push(format!(
            "{} in {}:{} is {} lines > {}",
            h.metric.name,
            h.metric.file,
            h.metric.start_line,
            h.metric.lines,
            opts.max_function_lines
        ));
    }
    for t in r.tools.iter().filter(|t| t.status == "failed") {
        r.reasons
            .push(format!("{} failed to run: {}", t.tool, t.detail));
    }
    let failed = r.tools.iter().any(|t| t.status == "failed");
    let blocking = lint_high > 0 || too_complex > 0 || too_long > 0 || organization_failed;
    let covered = r.functions_analyzed > 0
        || r.tools.iter().any(|t| t.status == "ran")
        || r.organization
            .as_ref()
            .is_some_and(|report| report.policy_valid && !report.files.is_empty());
    if organization_failed {
        r.quality_score = None;
        r.risk_level = "high".into();
    }
    if failed || !covered {
        r.quality_score = None;
        r.risk_level = "unknown".into();
        r.reasons.push("code-quality coverage is incomplete".into());
    }

    r.approval = if blocking {
        Some(false)
    } else if failed || !covered {
        None
    } else {
        Some(true)
    };
    r.duration_ms = t0.elapsed().as_millis() as u64;
    r
}

// ================================================================== python harness (desktop + resource)

pub async fn run_py_harness(cfg: &Config, input: &Value, timeout_s: u64) -> Result<Value, String> {
    let script = cfg.harness_py.join("desktop_test.py");
    if !script.exists() {
        return Err(format!("desktop harness not found at {}", script.display()));
    }
    let mut cmd = tokio::process::Command::new(&cfg.uv_exe);
    cmd.args(["run", "--quiet", "--project"])
        .arg(&cfg.harness_py)
        .arg("python")
        .arg(&script)
        .current_dir(&cfg.harness_py);
    let tool = if input["mode"] == "resource" {
        "resource_critic"
    } else {
        "desktop_test"
    };
    let out = crate::proc::run_streaming(cmd, input, timeout_s, tool).await?;
    serde_json::from_str(out.stdout.trim()).map_err(|e| {
        format!(
            "harness output not JSON: {e}; stderr: {}",
            out.stderr_tail.chars().take(300).collect::<String>()
        )
    })
}

// ================================================================== desktop_test

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct DesktopResult {
    pub tool: String,
    pub deterministic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub summary: BTreeMap<String, usize>,
    pub breakages: Vec<String>,
    pub approval: Option<bool>,
    pub out_dir: String,
    /// Harness report trimmed to problem interactions; complete report in `full_result`.
    pub report: Value,
    pub duration_ms: u64,
    pub full_result: Option<String>,
}

/// Pure: harness report -> summary, breakages, approval.
pub fn judge_desktop(
    report: &Value,
    ignore: &[String],
) -> (BTreeMap<String, usize>, Vec<String>, Option<bool>) {
    judge_desktop_with(report, ignore, None, None)
}

/// As `judge_desktop`, plus Unity soak judging (Player.log, input steps, frame budgets).
pub fn judge_desktop_with(
    report: &Value,
    ignore: &[String],
    min_avg_fps: Option<f64>,
    max_stutter_pct: Option<f64>,
) -> (BTreeMap<String, usize>, Vec<String>, Option<bool>) {
    let mut s = BTreeMap::new();
    let mut b = Vec::new();
    if report["ok"] != true {
        // a crash during startup is still a definite failure
        if report["crashes"].as_array().is_some_and(|c| !c.is_empty()) {
            b.push(format!(
                "app failed to start: {}",
                report["error"].as_str().unwrap_or("?")
            ));
            return (s, b, Some(false));
        }
        return (s, b, None);
    }
    let ignored = |n: &str| {
        ignore
            .iter()
            .any(|i| !i.is_empty() && n.contains(i.as_str()))
    };
    for i in report["interactions"].as_array().into_iter().flatten() {
        let res = i["result"].as_str().unwrap_or("");
        *s.entry(res.to_string()).or_default() += 1;
        let name = i["element"]["name"].as_str().unwrap_or("");
        let ct = i["element"]["control_type"].as_str().unwrap_or("");
        match res {
            "no_effect" if !ignored(name) => {
                *s.entry("unverified_effects".into()).or_default() += 1;
            }
            "error_dialog" | "crash" | "hang" if !ignored(name) => {
                let errs: Vec<&str> = i["errors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                b.push(format!("{res} after {ct} \"{name}\": {}", errs.join(" | ")));
            }
            _ => {}
        }
    }
    for p in report["persistence"].as_array().into_iter().flatten() {
        if p["persisted"] != true {
            b.push(format!(
                "value did not persist across restart: {} (expected {}, got {}{})",
                p["auto_id"].as_str().or(p["name"].as_str()).unwrap_or("?"),
                p["expected"],
                p["actual"],
                p["error"]
                    .as_str()
                    .map(|e| format!(", {e}"))
                    .unwrap_or_default()
            ));
        }
    }
    b.extend(crate::ui::judge_flows(
        report["flows"].as_array().map(Vec::as_slice).unwrap_or(&[]),
    ));
    for c in report["crashes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["after"].as_str().is_some_and(|a| a.starts_with("soak")))
    {
        b.push(format!(
            "crashed during {} (exit {}{})",
            c["after"].as_str().unwrap_or("soak"),
            c["exit_code"],
            c["detail"]
                .as_str()
                .map(|d| format!(", {d}"))
                .unwrap_or_default()
        ));
    }
    for h in report["hangs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|h| h["after"].as_str().is_some_and(|a| a.starts_with("soak")))
    {
        b.push(format!(
            "not responding during {} for {}s",
            h["after"].as_str().unwrap_or("soak"),
            h["seconds"]
        ));
    }
    let orphans: Vec<&str> = report["orphans"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| o.as_str())
        .collect();
    if !orphans.is_empty() {
        b.push(format!(
            "child process(es) outlived the app and were killed: {}",
            orphans.join(", ")
        ));
    }
    b.extend(crate::hygiene::judge_leaks(&report["hygiene"]));
    if report["unity"].is_object() {
        b.extend(crate::unity::judge_unity(
            &report["unity"],
            min_avg_fps,
            max_stutter_pct,
        ));
    }
    let stderr = report["stderr_tail"].as_str().unwrap_or("");
    let err_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| regex_err().is_match(l))
        .take(5)
        .collect();
    for l in err_lines {
        b.push(format!(
            "stderr: {}",
            l.chars().take(200).collect::<String>()
        ));
    }
    let has_evidence = report["interactions"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
        || report["flows"].as_array().is_some_and(|a| !a.is_empty())
        || report["windows"].as_array().is_some_and(|a| !a.is_empty())
        || report["unity"].is_object();
    let approval = if !b.is_empty() {
        Some(false)
    } else if s.get("unverified_effects").is_some_and(|count| *count > 0) {
        None
    } else if has_evidence {
        Some(true)
    } else {
        None
    };
    (s, b, approval)
}

fn regex_err() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(error|exception|traceback|fatal|panic|unhandled)\b").unwrap()
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn desktop_test(
    cfg: &Config,
    exe: &str,
    args: &[String],
    cwd: Option<&str>,
    window_title_re: Option<&str>,
    persist: &[Value],
    max_actions: usize,
    allow_destructive: bool,
    use_mouse: bool,
    hang_timeout_s: f64,
    ignore: &[String],
    flows: &[Value],
    explore: bool,
    unity: &Value,
) -> DesktopResult {
    let out_dir = crate::vision::run_dir(cfg, "desktop");
    let input = json!({
        "mode": "desktop_test", "exe": exe, "args": args, "cwd": cwd, "window_title_re": window_title_re,
        "persist": persist, "max_actions": max_actions, "allow_destructive": allow_destructive,
        "use_mouse": use_mouse, "hang_timeout_s": hang_timeout_s, "out_dir": out_dir,
        "flows": flows, "explore": explore, "tools_bin": cfg.tools_bin,
    });
    let mut input = input;
    if let (Some(dst), Some(src)) = (input.as_object_mut(), unity.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    let t0 = Instant::now();
    let mut r = DesktopResult {
        tool: "desktop_test".into(),
        deterministic: true,
        out_dir: out_dir.display().to_string(),
        ..Default::default()
    };
    match run_py_harness(cfg, &input, cfg.ui_timeout_s).await {
        Err(e) => r.error = Some(e),
        Ok(rep) => {
            if rep["ok"] != true {
                r.error = rep["error"].as_str().map(String::from);
            }
            let (s, b, a) = judge_desktop_with(
                &rep,
                ignore,
                unity["min_avg_fps"].as_f64(),
                unity["max_stutter_pct"].as_f64(),
            );
            r.summary = s;
            r.breakages = b;
            r.approval = a;
            r.report = rep;
        }
    }
    r.duration_ms = t0.elapsed().as_millis() as u64;
    r
}

// ================================================================== resource_critic

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct Budgets {
    pub max_cpu_avg_pct: Option<f64>,
    pub max_rss_mb: Option<f64>,
    pub max_rss_slope_mb_per_min: Option<f64>,
    pub max_handles_growth: Option<i64>,
    pub max_gpu_util_avg_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct ResourceResult {
    pub tool: String,
    pub advisory: bool,
    /// Processes and ports the run started and left behind (killed after reporting).
    #[serde(skip_serializing_if = "Value::is_null")]
    pub hygiene: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Measured by psutil / nvidia-smi.
    pub stats: Value,
    pub timeline: Value,
    pub budgets: Budgets,
    pub budget_violations: Vec<String>,
    /// Kev on the measured stats: low | medium | high, with probabilities.
    /// Kev: p(resource usage is reasonable for the described workload).
    pub p_reasonable: Option<f64>,
    /// 0-10 from Kev: 10 * p_reasonable.
    pub efficiency_score: Option<f64>,
    pub kev_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kev_error: Option<String>,
    pub approval: Option<bool>,
    pub reasons: Vec<String>,
    pub duration_ms: u64,
    pub full_result: Option<String>,
}

/// Deterministic budget checks. Defaults: RSS growth > 5 MB/min over >= 15 s, handles +500.
pub fn check_budgets(stats: &Value, b: &Budgets) -> Vec<String> {
    let f = |k: &str| stats[k].as_f64();
    let mut v = Vec::new();
    type Cap = (Option<f64>, &'static str, fn(f64, f64) -> String);
    let caps: [Cap; 3] = [
        (b.max_cpu_avg_pct, "cpu_avg_pct", |x, m| {
            format!("CPU avg {x:.0}% > budget {m:.0}% (100% = one core)")
        }),
        (b.max_rss_mb, "rss_peak_mb", |x, m| {
            format!("peak memory {x:.0} MB > budget {m:.0} MB")
        }),
        (b.max_gpu_util_avg_pct, "gpu_util_avg_pct", |x, m| {
            format!("GPU util avg {x:.0}% > budget {m:.0}% (whole-GPU)")
        }),
    ];
    for (max, key, msg) in caps {
        if let (Some(m), Some(x)) = (max, f(key))
            && x > m
        {
            v.push(msg(x, m));
        }
    }
    let slope_max = b.max_rss_slope_mb_per_min.unwrap_or(5.0);
    if let (Some(x), Some(dur)) = (f("rss_slope_mb_per_min"), f("duration_s"))
        && dur >= 15.0
        && x > slope_max
    {
        v.push(format!(
            "memory growing {x:.1} MB/min > {slope_max:.1} (possible leak)"
        ));
    }
    let handles =
        stats["handles_end"].as_i64().unwrap_or(0) - stats["handles_start"].as_i64().unwrap_or(0);
    let hmax = b.max_handles_growth.unwrap_or(500);
    if handles > hmax {
        v.push(format!(
            "handle count grew by {handles} > {hmax} (possible handle leak)"
        ));
    }
    if stats["exited_early"] == true {
        v.push(format!(
            "process exited during sampling (code {})",
            stats["exit_code"]
        ));
    }
    v
}

/// Missing measurements prevent approval; violations stay definite regardless of gaps.
pub fn resource_budget_gaps(stats: &Value, b: &Budgets) -> Vec<String> {
    let mut gaps = Vec::new();
    for (requested, key) in [
        (b.max_cpu_avg_pct.is_some(), "cpu_avg_pct"),
        (b.max_rss_mb.is_some(), "rss_peak_mb"),
        (b.max_gpu_util_avg_pct.is_some(), "gpu_util_avg_pct"),
    ] {
        if requested && !stats[key].as_f64().is_some_and(f64::is_finite) {
            gaps.push(format!("requested budget measurement unavailable: {key}"));
        }
    }
    if !stats["rss_slope_mb_per_min"]
        .as_f64()
        .is_some_and(f64::is_finite)
        || !stats["duration_s"].as_f64().is_some_and(|d| d >= 15.0)
    {
        gaps.push("RSS growth budget needs at least 15 seconds of measured observation".into());
    }
    if stats["handles_start"].as_i64().is_none() || stats["handles_end"].as_i64().is_none() {
        gaps.push("handle growth measurement unavailable".into());
    }
    if !stats["samples"].as_u64().is_some_and(|n| n >= 2) {
        gaps.push("fewer than two process samples; no meaningful resource coverage".into());
    }
    gaps
}

#[allow(clippy::too_many_arguments)]
pub async fn resource<D: Decider>(
    d: &D,
    cfg: &Config,
    command: Option<Value>,
    pid: Option<u32>,
    allow_attach: bool,
    cwd: Option<&str>,
    duration_s: f64,
    workload: &str,
    budgets: Budgets,
) -> ResourceResult {
    let t0 = Instant::now();
    let mut r = ResourceResult {
        tool: "resource_critic".into(),
        advisory: true,
        budgets: budgets.clone(),
        ..Default::default()
    };
    let _ = cfg;
    // Whatever the run starts must be gone when it ends: snapshot, run, then report leaks.
    let roots = crate::native::hygiene::roots_for(None, command.as_ref(), cwd, &[]);
    let before = (!roots.is_empty()).then(|| crate::native::hygiene::snapshot(&roots));
    let mut rep = crate::native::resource::resource(
        command.clone(),
        pid,
        allow_attach,
        cwd.map(String::from),
        duration_s,
        500,
        1.0,
    )
    .await;
    if let Some(b) = before
        && rep["ok"] == true
    {
        rep["hygiene"] =
            tokio::task::spawn_blocking(move || crate::native::hygiene::leaks(&b, true))
                .await
                .unwrap_or(Value::Null);
    }
    if rep["ok"] != true {
        r.error = rep["error"].as_str().map(String::from);
        return r;
    }
    r.stats = rep["stats"].clone();
    r.timeline = rep["timeline"].clone();
    r.budget_violations = check_budgets(&r.stats, &budgets);
    // Leaks are measured (budgets), not asked: Kev called a 1,120 MB/min leak "low risk".
    // Kev only says whether the use fits the described work (spec resource_fit, advisory).
    r.reasons = r.budget_violations.clone();
    r.reasons
        .extend(crate::hygiene::judge_leaks(&rep["hygiene"]));
    r.hygiene = rep["hygiene"].clone();
    if workload.trim().is_empty() {
        r.kev_ok = None;
    } else {
        let spec = crate::spec::get("resource_fit");
        match spec.state(&[
            ("workload", workload.chars().take(500).collect()),
            ("usage", usage_words(&r.stats)),
        ]) {
            Err(e) => {
                r.kev_ok = Some(false);
                r.kev_error = Some(e);
            }
            Ok(state) => match d.decide(&state, &spec.wire()).await {
                Ok(dec) => {
                    r.kev_ok = Some(true);
                    r.p_reasonable = Some(round2(dec.answers["fits"].value));
                    r.efficiency_score = r.p_reasonable.map(|p| round2(p * 10.0));
                }
                Err(e) => {
                    r.kev_ok = Some(false);
                    r.kev_error = Some(e);
                }
            },
        }
    }
    let gaps = resource_budget_gaps(&r.stats, &budgets);
    r.approval = if !r.reasons.is_empty() {
        Some(false)
    } else if !gaps.is_empty() {
        None
    } else {
        Some(true)
    };
    r.reasons.extend(gaps);
    r.duration_ms = t0.elapsed().as_millis() as u64;
    r
}

pub fn base_dir(base: Option<&str>) -> PathBuf {
    base.map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

/// The resource_fit spec's `usage` field: measured values in fixed words, written by code.
pub fn usage_words(stats: &Value) -> String {
    let n = |k: &str| stats[k].as_f64().unwrap_or(0.0);
    let trend = match n("rss_slope_mb_per_min") {
        x if x > 5.0 => "GROWING",
        x if x < -5.0 => "SHRINKING",
        _ => "FLAT",
    };
    let mut out = format!(
        "CPU: {:.0}% of one core on average, {:.0}% at peak. Memory: {:.0} MB at the end, {:.0} MB peak, {trend} ({:+.1} MB/min). Handles: {} to {}. Disk: {:.1} MB read, {:.1} MB written.",
        n("cpu_avg_pct"),
        n("cpu_max_pct"),
        n("rss_end_mb"),
        n("rss_peak_mb"),
        n("rss_slope_mb_per_min"),
        n("handles_start"),
        n("handles_end"),
        n("read_mb"),
        n("write_mb")
    );
    if let Some(g) = stats["gpu_util_avg_pct"].as_f64() {
        out.push_str(&format!(" GPU (whole card): {g:.0}% on average."));
    }
    out.chars().take(600).collect()
}

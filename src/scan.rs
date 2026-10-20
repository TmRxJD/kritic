//! External analyzers (security scanners, linters) normalized into one Finding shape. Every
//! finding comes from a real tool's JSON output; a tool that isn't installed or doesn't apply is
//! reported as skipped with a reason, never silently treated as clean.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, PartialEq)]
pub struct Finding {
    pub tool: String,
    pub rule: String,
    /// critical | high | medium | low | info
    pub severity: String,
    pub file: String,
    pub line: u64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ToolRun {
    pub tool: String,
    /// ran | skipped | failed
    pub status: String,
    pub detail: String,
    pub findings: usize,
    pub duration_ms: u64,
}

pub fn severity_rank(s: &str) -> u8 {
    match s {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

pub fn cvss_to_severity(score: f64) -> &'static str {
    if score >= 9.0 {
        "critical"
    } else if score >= 7.0 {
        "high"
    } else if score >= 4.0 {
        "medium"
    } else if score > 0.0 {
        "low"
    } else {
        "info"
    }
}

/// Where scanner binaries live (`scan.tools_bin`, default `~/.kritic/bin`, then PATH).
pub fn tool_path(bin_dir: &Path, name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) && Path::new(name).extension().is_none() {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let p = bin_dir.join(&exe);
    if p.is_file() {
        return Some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
}

pub async fn run_json(
    exe: &Path,
    args: &[&str],
    dir: &Path,
    timeout_s: u64,
) -> Result<(String, i32), String> {
    let (stdout, code, _) = run_output(exe, args, dir, timeout_s).await?;
    Ok((stdout, code))
}

async fn run_output(
    exe: &Path,
    args: &[&str],
    dir: &Path,
    timeout_s: u64,
) -> Result<(String, i32, String), String> {
    let child = tokio::process::Command::new(exe)
        .args(args)
        .current_dir(dir)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    let out = tokio::time::timeout(Duration::from_secs(timeout_s), child.wait_with_output())
        .await
        .map_err(|_| format!("timed out after {timeout_s}s"))?
        .map_err(|e| e.to_string())?;
    Ok((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr)
            .chars()
            .take(2000)
            .collect(),
    ))
}

fn rel(base: &Path, f: &str) -> String {
    let p = Path::new(f);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    full.strip_prefix(base)
        .unwrap_or(&full)
        .display()
        .to_string()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

// ------------------------------------------------------------------ parsers (pure, tested)

pub fn parse_bandit(base: &Path, json: &str) -> Result<Vec<Finding>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bandit JSON: {e}"))?;
    Ok(v["results"]
        .as_array()
        .ok_or("bandit: no results array")?
        .iter()
        .map(|r| Finding {
            tool: "bandit".into(),
            rule: format!(
                "{} {}",
                r["test_id"].as_str().unwrap_or(""),
                r["test_name"].as_str().unwrap_or("")
            ),
            severity: r["issue_severity"]
                .as_str()
                .unwrap_or("low")
                .to_ascii_lowercase(),
            file: rel(base, r["filename"].as_str().unwrap_or("")),
            line: r["line_number"].as_u64().unwrap_or(0),
            message: r["issue_text"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(300)
                .collect(),
            confidence: r["issue_confidence"]
                .as_str()
                .map(|s| s.to_ascii_lowercase()),
        })
        .collect())
}

pub fn parse_semgrep(base: &Path, json: &str) -> Result<Vec<Finding>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("semgrep JSON: {e}"))?;
    Ok(v["results"]
        .as_array()
        .ok_or("semgrep: no results array")?
        .iter()
        .map(|r| {
            let sev = match r["extra"]["severity"].as_str().unwrap_or("INFO") {
                "ERROR" => "high",
                "WARNING" => "medium",
                _ => "low",
            };
            Finding {
                tool: "semgrep".into(),
                rule: r["check_id"]
                    .as_str()
                    .unwrap_or("")
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .to_string(),
                severity: sev.into(),
                file: rel(base, r["path"].as_str().unwrap_or("")),
                line: r["start"]["line"].as_u64().unwrap_or(0),
                message: r["extra"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(300)
                    .collect(),
                confidence: r["extra"]["metadata"]["confidence"]
                    .as_str()
                    .map(|s| s.to_ascii_lowercase()),
            }
        })
        .collect())
}

pub fn parse_gitleaks(base: &Path, json: &str) -> Result<Vec<Finding>, String> {
    let v: Value = serde_json::from_str(if json.trim().is_empty() { "[]" } else { json })
        .map_err(|e| format!("gitleaks JSON: {e}"))?;
    Ok(v.as_array()
        .ok_or("gitleaks: expected array")?
        .iter()
        .map(|f| Finding {
            tool: "gitleaks".into(),
            rule: f["RuleID"].as_str().unwrap_or("secret").into(),
            // a committed credential is always treated as high
            severity: "high".into(),
            file: rel(base, f["File"].as_str().unwrap_or("")),
            line: f["StartLine"].as_u64().unwrap_or(0),
            message: format!(
                "{} (secret redacted)",
                f["Description"].as_str().unwrap_or("possible secret")
            ),
            confidence: None,
        })
        .collect())
}

/// Maintenance notices are preserved, but are not mislabeled as exploitable vulnerabilities.
/// An unknown category, mixed affected metadata, or explicit severity stays a security advisory.
fn informational_advisory(v: &Value) -> bool {
    let no_severity = v
        .get("severity")
        .is_none_or(|s| s.as_array().is_some_and(Vec::is_empty));
    no_severity
        && v["affected"].as_array().is_some_and(|affected| {
            !affected.is_empty()
                && affected.iter().all(|a| {
                    matches!(
                        a["database_specific"]["informational"].as_str(),
                        Some("unmaintained" | "notice")
                    )
                })
        })
}

/// osv-scanner v2 `scan source --format json`. One finding per vulnerable package, at the
/// package's worst group severity (CVSS), listing its advisory ids.
/// Retain manifest provenance relative to the scan root, as with the other analyzers.
pub fn parse_osv(base: &Path, json: &str) -> Result<Vec<Finding>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("osv JSON: {e}"))?;
    let mut out = Vec::new();
    for r in v["results"].as_array().ok_or("osv: no results array")? {
        let src = r["source"]["path"].as_str().unwrap_or("").to_string();
        for p in r["packages"].as_array().ok_or("osv: no packages array")? {
            let vulns = p["vulnerabilities"].as_array().map(Vec::len).unwrap_or(0);
            if vulns == 0 {
                continue;
            }
            let worst = p["groups"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| {
                    g["max_severity"]
                        .as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                })
                .fold(0.0, f64::max);
            let informational = worst == 0.0
                && p["vulnerabilities"].as_array().is_some_and(|items| {
                    !items.is_empty() && items.iter().all(informational_advisory)
                });
            let ids: Vec<&str> = p["vulnerabilities"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x["id"].as_str())
                .take(5)
                .collect();
            let name = p["package"]["name"].as_str().unwrap_or("?");
            let ver = p["package"]["version"].as_str().unwrap_or("?");
            out.push(Finding {
                tool: "osv-scanner".into(),
                rule: if informational {
                    "informational-dependency"
                } else {
                    "vulnerable-dependency"
                }
                .into(),
                severity: if informational {
                    "info".into()
                } else if worst > 0.0 {
                    cvss_to_severity(worst).into()
                } else {
                    "medium".into()
                },
                file: rel(base, &src),
                line: 0,
                message: format!(
                    "{name} {ver}: {vulns} {} ({}), e.g. {}",
                    if informational {
                        "informational advisories"
                    } else {
                        "security advisories"
                    },
                    if worst > 0.0 {
                        format!("worst CVSS {worst:.1}")
                    } else {
                        "no scored CVSS supplied".into()
                    },
                    ids.join(", ")
                ),
                confidence: None,
            });
        }
    }
    Ok(out)
}

pub fn parse_ruff(base: &Path, json: &str) -> Result<Vec<Finding>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("ruff JSON: {e}"))?;
    Ok(v.as_array()
        .ok_or("ruff: expected array")?
        .iter()
        .map(|d| {
            let code = d["code"].as_str().unwrap_or("");
            // syntax errors and undefined/redefined names break at runtime; pyflakes/bugbear are
            // likely bugs; everything else is style
            let sev = if code.starts_with("E9") || code == "F821" || code == "F811" {
                "high"
            } else if code.starts_with('F') || code.starts_with('B') {
                "medium"
            } else {
                "low"
            };
            Finding {
                tool: "ruff".into(),
                rule: code.into(),
                severity: sev.into(),
                file: rel(base, d["filename"].as_str().unwrap_or("")),
                line: d["location"]["row"].as_u64().unwrap_or(0),
                message: d["message"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(300)
                    .collect(),
                confidence: None,
            }
        })
        .collect())
}

/// cargo clippy --message-format json: one JSON object per line.
pub fn parse_clippy(base: &Path, stdout: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["reason"] != "compiler-message" {
            continue;
        }
        let m = &v["message"];
        let level = m["level"].as_str().unwrap_or("");
        if level != "warning" && level != "error" {
            continue;
        }
        let span = m["spans"]
            .as_array()
            .and_then(|s| s.iter().find(|x| x["is_primary"] == true).or(s.first()));
        let Some(span) = span else { continue };
        out.push(Finding {
            tool: "clippy".into(),
            rule: m["code"]["code"].as_str().unwrap_or("").into(),
            severity: if level == "error" {
                "high".into()
            } else {
                "low".into()
            },
            file: rel(base, span["file_name"].as_str().unwrap_or("")),
            line: span["line_start"].as_u64().unwrap_or(0),
            message: m["message"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(300)
                .collect(),
            confidence: None,
        });
    }
    out.dedup();
    out
}

pub fn parse_eslint(base: &Path, json: &str) -> Result<Vec<Finding>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("eslint JSON: {e}"))?;
    let mut out = Vec::new();
    for f in v.as_array().ok_or("eslint: expected array")? {
        for m in f["messages"]
            .as_array()
            .ok_or("eslint: no messages array")?
        {
            out.push(Finding {
                tool: "eslint".into(),
                rule: m["ruleId"].as_str().unwrap_or("parse").into(),
                severity: if m["severity"] == 2 {
                    "high".into()
                } else {
                    "low".into()
                },
                file: rel(base, f["filePath"].as_str().unwrap_or("")),
                line: m["line"].as_u64().unwrap_or(0),
                message: m["message"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(300)
                    .collect(),
                confidence: None,
            });
        }
    }
    Ok(out)
}

/// `tsc --noEmit --pretty false`: `file(line,col): error TS1234: message`
pub fn parse_tsc(base: &Path, stdout: &str) -> Vec<Finding> {
    let re = regex::Regex::new(r"^(.+?)\((\d+),\d+\): (error|warning) (TS\d+): (.*)$").unwrap();
    stdout
        .lines()
        .filter_map(|l| re.captures(l.trim()))
        .map(|c| Finding {
            tool: "tsc".into(),
            rule: c[4].into(),
            severity: if &c[3] == "error" {
                "high".into()
            } else {
                "low".into()
            },
            file: rel(base, &c[1]),
            line: c[2].parse().unwrap_or(0),
            message: c[5].chars().take(300).collect(),
            confidence: None,
        })
        .collect()
}

/// Third-party / generated trees that must never be scanned as the project's own code.
pub const VENDORED: &[&str] = &[
    "node_modules",
    ".venv",
    "venv",
    "site-packages",
    "target",
    "dist",
    "build",
    ".git",
    "__pycache__",
    ".next",
    ".svelte-kit",
    "coverage",
    "vendor",
    "third_party",
    "graphify-out",
];

/// True if any path component is a vendored/generated directory.
pub fn is_vendored(file: &str) -> bool {
    file.split(['/', '\\']).any(|c| VENDORED.contains(&c))
}

/// bandit `-x` globs for every vendored dir at any depth.
pub fn bandit_excludes() -> String {
    VENDORED
        .iter()
        .map(|d| format!("*/{d}/*"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A gitleaks config that keeps the default rules and allowlists vendored paths (gitleaks `dir`
/// mode does not honor .gitignore). Written to `dir`; returns its path.
pub fn gitleaks_config(dir: &Path) -> std::io::Result<PathBuf> {
    let alts = VENDORED
        .iter()
        .map(|d| regex::escape(d))
        .collect::<Vec<_>>()
        .join("|");
    // TOML literal string: backslashes reach the regex unescaped, so `[\\/]` matches either separator
    let body = format!(
        r#"[extend]
useDefault = true

[allowlist]
description = "vendored and generated trees"
paths = ['(^|[\\/])({alts})[\\/]']

[[rules]]
id = "generic-api-key"
[[rules.allowlists]]
description = "ACS decision content identity, only in the canonical_key JSON field"
regexTarget = "match"
regexes = ['^"?canonical_key"\s*:\s*"acs-decision-[a-f0-9]{{64}}"[\s,}}]*$']
"#
    );
    let path = dir.join("kritic-gitleaks.toml");
    std::fs::write(&path, body)?;
    Ok(path)
}

// ------------------------------------------------------------------ project detection

#[derive(Debug, Default, Clone)]
pub struct Project {
    pub python: bool,
    pub rust: bool,
    pub node: bool,
    pub eslint: bool,
    pub tsconfig: bool,
    pub lockfiles: bool,
}

pub fn detect(base: &Path) -> Project {
    let has = |n: &str| base.join(n).exists();
    let mut py = false;
    let mut walk = vec![base.to_path_buf()];
    let mut seen = 0;
    while let Some(d) = walk.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            let p = e.path();
            let n = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !n.starts_with('.')
                    && ![
                        "node_modules",
                        "target",
                        "dist",
                        "build",
                        "__pycache__",
                        "venv",
                    ]
                    .contains(&n.as_str())
                {
                    walk.push(p);
                }
            } else if n.ends_with(".py") {
                py = true;
            }
        }
        if py || seen > 20_000 {
            break;
        }
    }
    Project {
        python: py || has("pyproject.toml") || has("requirements.txt"),
        rust: has("Cargo.toml"),
        node: has("package.json"),
        eslint: [
            "eslint.config.js",
            "eslint.config.mjs",
            "eslint.config.cjs",
            ".eslintrc.json",
            ".eslintrc.js",
            ".eslintrc.cjs",
            ".eslintrc",
        ]
        .iter()
        .any(|f| has(f)),
        tsconfig: has("tsconfig.json"),
        lockfiles: [
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "bun.lock",
            "Cargo.lock",
            "requirements.txt",
            "poetry.lock",
            "uv.lock",
            "go.sum",
            "Gemfile.lock",
            "composer.lock",
            "pom.xml",
        ]
        .iter()
        .any(|f| has(f)),
    }
}

pub struct Runner<'a> {
    pub base: &'a Path,
    pub bin_dir: &'a Path,
    pub timeout_s: u64,
    pub runs: Vec<ToolRun>,
    pub findings: Vec<Finding>,
}

/// Validate completion independently from extracted findings: partial findings remain useful,
/// but a broken scan must never be recorded as clean.
pub fn completion_error(tool: &str, stdout: &str, code: i32, findings: usize) -> Option<String> {
    let allowed = code == 0
        || (code == 1
            && findings > 0
            && matches!(
                tool,
                "bandit" | "osv-scanner" | "eslint" | "cargo-audit" | "semgrep"
            ));
    if !allowed {
        return Some(format!("{tool} exited with {code}; scan incomplete"));
    }
    if tool == "tsc" {
        return (!stdout.trim().is_empty() && findings == 0)
            .then(|| "tsc returned unrecognized diagnostics".into());
    }
    if tool == "clippy" {
        let mut finished = false;
        for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return Some("clippy returned non-JSON output".into());
            };
            if v["reason"] == "build-finished" {
                if v["success"] != true {
                    return Some("clippy build did not succeed".into());
                }
                finished = true;
            }
        }
        return (!finished).then(|| "clippy did not report a completed build".into());
    }
    // Gitleaks emits empty stdout on some successful, finding-free versions.
    if tool == "gitleaks" && stdout.trim().is_empty() {
        return None;
    }
    let Ok(v) = serde_json::from_str::<Value>(stdout) else {
        return Some(format!("{tool} returned invalid JSON"));
    };
    if let Some(errors) = v.get("errors")
        && !errors.as_array().is_some_and(|a| a.is_empty())
    {
        let details = errors
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .take(3)
                    .map(|error| {
                        ["type", "path", "message"]
                            .iter()
                            .filter_map(|key| error.get(key).and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(": ")
                            .chars()
                            .take(300)
                            .collect::<String>()
                    })
                    .filter(|detail| !detail.is_empty())
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();
        return Some(if details.is_empty() {
            format!("{tool} report contains scanner errors")
        } else {
            format!("{tool} report contains scanner errors: {details}")
        });
    }
    let valid = match tool {
        "bandit" | "semgrep" | "osv-scanner" => v["results"].is_array(),
        "gitleaks" | "ruff" | "eslint" => v.is_array(),
        "cargo-audit" => v["vulnerabilities"]["list"].is_array(),
        _ => true,
    };
    if !valid {
        return Some(format!("{tool} report is missing required result fields"));
    }
    if tool == "semgrep"
        && !v["paths"]["scanned"]
            .as_array()
            .is_some_and(|paths| !paths.is_empty())
    {
        return Some("semgrep reported no scanned files; coverage unverified".into());
    }
    if tool == "bandit"
        && !v["metrics"]["_totals"]["loc"]
            .as_u64()
            .is_some_and(|loc| loc > 0)
    {
        return Some("bandit reported no analyzed lines; coverage unverified".into());
    }
    let fields: &[&str] = match tool {
        "bandit" => &["/test_id", "/filename", "/issue_severity", "/issue_text"],
        "semgrep" => &["/check_id", "/path", "/extra/severity", "/extra/message"],
        "gitleaks" => &["/RuleID", "/File", "/Description"],
        "ruff" => &["/filename", "/message"],
        "eslint" => &["/filePath"],
        _ => &[],
    };
    let rows = if matches!(tool, "bandit" | "semgrep") {
        v["results"].as_array()
    } else {
        v.as_array()
    };
    if let Some(rows) = rows {
        for row in rows {
            if fields
                .iter()
                .any(|path| !row.pointer(path).is_some_and(Value::is_string))
            {
                return Some(format!("{tool} contains a malformed result"));
            }
            if tool == "eslint"
                && !row["messages"].as_array().is_some_and(|messages| {
                    messages.iter().all(|m| {
                        m["message"].is_string() && matches!(m["severity"].as_u64(), Some(1 | 2))
                    })
                })
            {
                return Some("eslint contains malformed diagnostics".into());
            }
        }
    }
    None
}

impl<'a> Runner<'a> {
    pub fn new(base: &'a Path, bin_dir: &'a Path, timeout_s: u64) -> Self {
        Runner {
            base,
            bin_dir,
            timeout_s,
            runs: Vec::new(),
            findings: Vec::new(),
        }
    }

    pub fn skip(&mut self, tool: &str, why: &str) {
        self.runs.push(ToolRun {
            tool: tool.into(),
            status: "skipped".into(),
            detail: why.into(),
            findings: 0,
            duration_ms: 0,
        });
    }

    /// Run `exe args`, parse with `parse(stdout)`, record the run.
    pub async fn run<F>(&mut self, tool: &str, exe: Option<PathBuf>, args: &[&str], parse: F)
    where
        F: FnOnce(&str) -> Result<Vec<Finding>, String>,
    {
        let Some(exe) = exe else {
            self.runs.push(ToolRun {
                tool: tool.into(),
                status: "failed".into(),
                detail: "applicable scanner is not installed".into(),
                findings: 0,
                duration_ms: 0,
            });
            return;
        };
        let t0 = Instant::now();
        let res = run_output(&exe, args, self.base, self.timeout_s).await;
        let ms = t0.elapsed().as_millis() as u64;
        match res {
            Ok((out, code, stderr)) => {
                let parsed = parse(&out);
                let mut error =
                    completion_error(tool, &out, code, parsed.as_ref().map_or(0, Vec::len));
                let f = match parsed {
                    Ok(f) => f,
                    Err(e) => {
                        error.get_or_insert(e);
                        Vec::new()
                    }
                };
                // backstop: drop anything a tool reported from a vendored tree
                let before = f.len();
                let f: Vec<Finding> = f.into_iter().filter(|x| !is_vendored(&x.file)).collect();
                let detail = if let Some(e) = &error {
                    // Gitleaks stderr can contain credentials; retain the failure without leaking them.
                    if tool == "gitleaks" || stderr.trim().is_empty() {
                        e.clone()
                    } else {
                        format!("{e}; stderr: {}", stderr.trim())
                    }
                } else if before > f.len() {
                    format!("{} findings in vendored paths ignored", before - f.len())
                } else {
                    String::new()
                };
                self.runs.push(ToolRun {
                    tool: tool.into(),
                    status: if error.is_some() { "failed" } else { "ran" }.into(),
                    detail,
                    findings: f.len(),
                    duration_ms: ms,
                });
                self.findings.extend(f);
            }
            Err(e) => self.runs.push(ToolRun {
                tool: tool.into(),
                status: "failed".into(),
                detail: e.chars().take(300).collect(),
                findings: 0,
                duration_ms: ms,
            }),
        }
    }
}

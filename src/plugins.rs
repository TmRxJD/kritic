//! Repo-local plugins: a project adds its own evaluators without changing kritic.
//!
//! Discovery: `<base_dir>/.kritic/plugins/*.toml|*.json`, one manifest per tool.
//! Contract: kritic runs `command` in the repo with JSON on stdin
//!   {"args": {...}, "base_dir": "...", "out_dir": "..."}
//! and expects one JSON object on stdout
//!   {"ok": bool, "approval"?: bool, "summary"?: str, "error"?: str,
//!    "findings"?: [{"severity": "critical|high|medium|low|info", "message": str, "file"?: str, "line"?: int}],
//!    "metrics"?: {...}, "kev_state"?: {"<field>": "<value>", ...}}
//! Progress: JSON lines with an "event" key on stderr are streamed to kritic's WebSocket.
//! Optional Kev judging: the manifest's `kev` block is a full decision spec (crate::spec): the
//! state fields Kev reads, each defined; the questions, each naming the fields it reads with
//! every answer defined; and per question a gate that may only fail approval with recorded
//! calibration evidence. The plugin returns `kev_state` with exactly those fields (code writes
//! measured values as vocabulary terms; Kev never sees raw metrics). The spec is linted when the
//! manifest is discovered. Approval: plugin approval (if given) AND no high/critical findings
//! (unless allowed) AND every calibrated Kev gate passes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Config;
use crate::kev::Decider;
use crate::spec::DecisionSpec;

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    /// argv; "{base_dir}" and "{out_dir}" are substituted. Relative program paths resolve against base_dir.
    pub command: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_s: u64,
    /// JSON Schema for `args` (documentation for the caller; required keys are checked).
    #[serde(default)]
    pub input_schema: Option<Value>,
    /// Allow high/critical findings without failing approval (e.g. report-only tools).
    #[serde(default)]
    pub findings_advisory: bool,
    /// Optional Kev judging, as a decision spec (see crate::spec and `spec_lint`).
    #[serde(default)]
    pub kev: Option<DecisionSpec>,
    /// Set by discovery.
    #[serde(default, skip_deserializing)]
    pub manifest_path: String,
}

fn default_timeout() -> u64 {
    300
}

pub fn plugin_dir(base: &Path) -> PathBuf {
    base.join(".kritic").join("plugins")
}

/// Load every manifest; invalid ones are returned as errors instead of being skipped silently.
pub fn discover(base: &Path) -> (Vec<Manifest>, Vec<String>) {
    let mut ok = Vec::new();
    let mut errs = Vec::new();
    let Ok(rd) = std::fs::read_dir(plugin_dir(base)) else {
        return (ok, errs);
    };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    files.sort();
    for p in files {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "toml" && ext != "json" {
            continue;
        }
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) => {
                errs.push(format!("{}: {e}", p.display()));
                continue;
            }
        };
        let parsed: Result<Manifest, String> = if ext == "toml" {
            toml::from_str(&text).map_err(|e| e.to_string())
        } else {
            serde_json::from_str(&text).map_err(|e| e.to_string())
        };
        match parsed.and_then(|m| validate(&m).map(|_| m)) {
            Ok(mut m) => {
                m.manifest_path = p.display().to_string();
                if ok.iter().any(|x: &Manifest| x.name == m.name) {
                    errs.push(format!(
                        "{}: duplicate plugin name '{}'",
                        p.display(),
                        m.name
                    ));
                } else {
                    ok.push(m);
                }
            }
            Err(e) => errs.push(format!("{}: {e}", p.display())),
        }
    }
    (ok, errs)
}

fn validate(m: &Manifest) -> Result<(), String> {
    if m.name.is_empty()
        || !m
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "invalid name '{}' (use letters, digits, _ or -)",
            m.name
        ));
    }
    if m.command.is_empty() {
        return Err("command is empty".into());
    }
    if let Some(k) = &m.kev {
        let errors = k.lint();
        if !errors.is_empty() {
            return Err(format!("kev spec: {}", errors.join("; ")));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct PluginResult {
    pub tool: String,
    pub plugin: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub summary: Option<String>,
    /// First 25; all of them in `full_result`.
    pub findings: Vec<Value>,
    pub finding_counts: BTreeMap<String, usize>,
    pub metrics: Value,
    /// Kev answers to the manifest's questions (probabilities / choice / score level).
    pub kev: Value,
    pub approval: Option<bool>,
    pub reasons: Vec<String>,
    pub duration_ms: u64,
    pub full_result: Option<String>,
}

fn rank(sev: &str) -> u8 {
    crate::scan::severity_rank(sev)
}

/// Pure: plugin output + Kev answers -> approval and reasons.
pub fn judge(
    m: &Manifest,
    out: &Value,
    kev: &BTreeMap<String, crate::kev::Answer>,
) -> (Option<bool>, Vec<String>) {
    let mut reasons = Vec::new();
    if out["ok"] != true {
        return (
            None,
            vec![format!(
                "plugin reported not ok: {}",
                out["error"].as_str().unwrap_or("no error given")
            )],
        );
    }
    if out["approval"] == false {
        reasons.push(format!(
            "plugin: {}",
            out["summary"].as_str().unwrap_or("not approved")
        ));
    }
    if !m.findings_advisory {
        for f in out["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|f| rank(f["severity"].as_str().unwrap_or("")) >= 3)
            .take(10)
        {
            reasons.push(format!(
                "[{}] {}{}",
                f["severity"].as_str().unwrap_or("?"),
                f["message"].as_str().unwrap_or(""),
                f["file"]
                    .as_str()
                    .map(|p| format!(
                        " ({p}{})",
                        f["line"]
                            .as_u64()
                            .map(|l| format!(":{l}"))
                            .unwrap_or_default()
                    ))
                    .unwrap_or_default()
            ));
        }
    }
    if let Some(spec) = &m.kev {
        for (id, a) in kev {
            reasons.extend(spec.check(id, a));
        }
    }
    (Some(reasons.is_empty()), reasons)
}

pub async fn run<D: Decider>(
    d: &D,
    cfg: &Config,
    base: &Path,
    name: &str,
    args: &Value,
) -> PluginResult {
    let t0 = Instant::now();
    let mut r = PluginResult {
        tool: "plugin_run".into(),
        plugin: name.into(),
        ..Default::default()
    };
    let (plugins, errs) = discover(base);
    let Some(m) = plugins.into_iter().find(|m| m.name == name) else {
        r.error = Some(format!(
            "no plugin '{name}' in {} {}",
            plugin_dir(base).display(),
            if errs.is_empty() {
                String::new()
            } else {
                format!("(manifest errors: {})", errs.join("; "))
            }
        ));
        return r;
    };
    if let Some(req) = m
        .input_schema
        .as_ref()
        .and_then(|s| s["required"].as_array())
    {
        let missing: Vec<&str> = req
            .iter()
            .filter_map(Value::as_str)
            .filter(|k| args.get(k).is_none())
            .collect();
        if !missing.is_empty() {
            r.error = Some(format!("missing required args: {}", missing.join(", ")));
            return r;
        }
    }
    let out_dir = crate::vision::run_dir(cfg, &format!("plugin-{name}"));
    let _ = std::fs::create_dir_all(&out_dir);
    let sub = |s: &str| {
        s.replace("{base_dir}", &base.display().to_string())
            .replace("{out_dir}", &out_dir.display().to_string())
    };
    let mut argv: Vec<String> = m.command.iter().map(|a| sub(a)).collect();
    let prog = PathBuf::from(&argv[0]);
    if prog.components().count() > 1 && prog.is_relative() {
        argv[0] = base.join(prog).display().to_string();
    }
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(
        m.cwd
            .as_deref()
            .map(|c| base.join(c))
            .unwrap_or_else(|| base.to_path_buf()),
    );
    let input = json!({"args": args, "base_dir": base, "out_dir": out_dir});
    let out = match crate::proc::run_streaming(cmd, &input, m.timeout_s, &format!("plugin:{name}"))
        .await
    {
        Ok(o) => o,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    let v: Value = match serde_json::from_str(out.stdout.trim()) {
        Ok(v) => v,
        Err(e) => {
            r.error = Some(format!(
                "plugin output is not one JSON object: {e}; stderr: {}",
                out.stderr_tail.chars().take(300).collect::<String>()
            ));
            return r;
        }
    };
    r.ok = v["ok"] == true;
    r.error = v["error"].as_str().map(String::from);
    r.summary = v["summary"].as_str().map(String::from);
    r.metrics = v["metrics"].clone();
    let findings: Vec<Value> = v["findings"].as_array().cloned().unwrap_or_default();
    for f in &findings {
        *r.finding_counts
            .entry(f["severity"].as_str().unwrap_or("info").to_string())
            .or_default() += 1;
    }
    // Kev on the plugin's declared state (never on raw summary/metrics)
    let mut answers = BTreeMap::new();
    if let Some(spec) = m.kev.as_ref().filter(|_| r.ok) {
        let given: Vec<(&str, String)> = v["kev_state"]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, x)| {
                        (
                            k.as_str(),
                            x.as_str()
                                .map(String::from)
                                .unwrap_or_else(|| x.to_string()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        match spec.state(&given) {
            Err(e) => r.reasons.push(format!(
                "Kev unavailable: plugin kev_state does not match its spec: {e}"
            )),
            Ok(state) => match d.decide(&state, &spec.wire()).await {
                Ok(dec) => {
                    answers = dec.answers;
                    r.kev = serde_json::to_value(&answers).unwrap_or_default();
                }
                Err(e) => r.reasons.push(format!("Kev unavailable: {e}")),
            },
        }
    }
    let (approval, reasons) = judge(&m, &v, &answers);
    r.approval =
        if r.reasons.iter().any(|x| x.starts_with("Kev unavailable")) && approval == Some(true) {
            None
        } else {
            approval
        };
    r.reasons.extend(reasons);
    r.findings = findings;
    r.full_result = crate::tools::save_full(cfg, &format!("plugin-{name}"), &r);
    r.findings.truncate(crate::tools::MAX_ITEMS);
    r.duration_ms = t0.elapsed().as_millis() as u64;
    r
}

//! Unity support: judging Unity-player soak reports, and running a project's Unity Test Framework
//! tests headless (`Unity.exe -batchmode -runTests`) with NUnit XML results parsed.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::config::Config;

/// Breakages from a Unity soak report (`report["unity"]`): Player.log problems and frame budgets.
pub fn judge_unity(u: &Value, min_avg_fps: Option<f64>, max_stutter_pct: Option<f64>) -> Vec<String> {
    let mut b = Vec::new();
    let counts = &u["log"]["counts"];
    for kind in ["native_crash", "exception", "assertion", "missing", "shader", "error"] {
        if let Some(n) = counts[kind].as_u64().filter(|n| *n > 0) {
            let first = u["log"]["samples"][kind][0].as_str().unwrap_or("");
            b.push(format!("Player.log: {n} {kind} line(s), e.g. {}", first.chars().take(200).collect::<String>()));
        }
    }
    for (i, s) in u["steps"].as_array().into_iter().flatten().enumerate() {
        if s["result"] == "fail" {
            b.push(format!("unity step {} ({}) failed: {}", i + 1, s["do"].as_str().unwrap_or("?"), s["error"].as_str().unwrap_or("?")));
        }
    }
    if let Some(busy) = u["preflight"]["port_busy"].as_str() {
        b.push(format!("app_events port already in use before launch: {busy} (app not launched)"));
    }
    if let Some(why) = u["ready"]["failed"].as_str() {
        b.push(format!("app failed while starting: {why}"));
    }
    if let Some(why) = u["ready"]["stalled"].as_str() {
        b.push(format!("app stalled while starting: {why}"));
    }
    if u["app_events"].is_object() && u["app_events"]["connected"] != true {
        b.push(format!(
            "app event stream {} never connected: {}",
            u["app_events"]["url"].as_str().unwrap_or("?"),
            u["app_events"]["error"].as_str().unwrap_or("no connection")
        ));
    }
    if u["ready"]["timed_out"] == true {
        b.push(format!("app never became ready (no ready marker within {}s)", u["ready"]["waited_s"]));
    }
    let f = &u["frames"];
    if f["ok"] == true {
        if let (Some(min), Some(fps)) = (min_avg_fps, f["avg_fps"].as_f64())
            && fps < min
        {
            b.push(format!("average {fps:.1} fps < {min:.0} (1% low {} fps, p99 {} ms)", f["low_1pct_fps"], f["p99_ms"]));
        }
        if let (Some(max), Some(pct)) = (max_stutter_pct, f["stutter_pct"].as_f64())
            && pct > max
        {
            b.push(format!("{pct:.1}% of frames stutter (> 2x median frame time) > {max:.1}%"));
        }
    } else if min_avg_fps.is_some() || max_stutter_pct.is_some() {
        b.push(format!("frame budgets set but no frame timing: {}", f["error"].as_str().unwrap_or("unknown")));
    }
    b
}

// ------------------------------------------------------------------ unity_tests

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct FailedTest {
    pub name: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct UnityTestResult {
    pub tool: String,
    pub deterministic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub editor: String,
    pub platform: String,
    pub total: u64,
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub failures: Vec<FailedTest>,
    pub results_xml: String,
    pub editor_log: String,
    /// Last lines of the editor log when the run itself failed (compile errors, licensing).
    pub log_tail: String,
    pub approval: Option<bool>,
    pub duration_ms: u64,
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"\b{name}="([^"]*)""#)).ok()?;
    re.captures(tag).map(|c| c[1].to_string())
}

/// Parse NUnit 3 XML as written by the Unity Test Framework.
pub fn parse_nunit(xml: &str) -> Result<(u64, u64, u64, u64, Vec<FailedTest>), String> {
    static RUN: OnceLock<Regex> = OnceLock::new();
    static CASE: OnceLock<Regex> = OnceLock::new();
    let run = RUN.get_or_init(|| Regex::new(r"<test-run\b[^>]*>").unwrap());
    let case = CASE.get_or_init(|| Regex::new(r#"(?s)<test-case\b([^>]*?)(/>|>(.*?)</test-case>)"#).unwrap());
    static MSG: OnceLock<Regex> = OnceLock::new();
    let msg_re = MSG.get_or_init(|| Regex::new(r"(?s)<message>\s*(?:<!\[CDATA\[)?(.*?)(?:\]\]>)?\s*</message>").unwrap());
    let head = run.find(xml).ok_or("no <test-run> element")?.as_str();
    let n = |k: &str| attr(head, k).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut failures = Vec::new();
    for c in case.captures_iter(xml) {
        let attrs = &c[1];
        if attr(attrs, "result").as_deref() == Some("Failed") {
            let body = c.get(3).map(|m| m.as_str()).unwrap_or("");
            let msg = msg_re
                .captures(body)
                .map(|m| m[1].trim().chars().take(400).collect())
                .unwrap_or_default();
            failures.push(FailedTest { name: attr(attrs, "fullname").or_else(|| attr(attrs, "name")).unwrap_or_default(), message: msg });
        }
    }
    Ok((n("total"), n("passed"), n("failed"), n("skipped"), failures))
}

/// Editor matching the project's ProjectVersion.txt, else KRITIC_UNITY_EDITOR.
pub fn find_editor(cfg: &Config, project: &Path) -> Result<PathBuf, String> {
    if let Some(e) = &cfg.unity_editor {
        return Ok(e.clone());
    }
    let pv = std::fs::read_to_string(project.join("ProjectSettings").join("ProjectVersion.txt"))
        .map_err(|e| format!("not a Unity project (ProjectSettings/ProjectVersion.txt): {e}"))?;
    let ver = pv
        .lines()
        .find_map(|l| l.strip_prefix("m_EditorVersion:"))
        .map(str::trim)
        .ok_or("m_EditorVersion missing in ProjectVersion.txt")?;
    let exe = cfg.unity_hub_editors.join(ver).join("Editor").join("Unity.exe");
    if exe.exists() {
        Ok(exe)
    } else {
        Err(format!("Unity {ver} not installed at {} (install it via Unity Hub or set KRITIC_UNITY_EDITOR)", exe.display()))
    }
}

pub async fn run_tests(cfg: &Config, project: &Path, platform: &str, filter: Option<&str>, timeout_s: u64) -> UnityTestResult {
    let t0 = Instant::now();
    let out = crate::vision::run_dir(cfg, "unity-tests");
    let _ = std::fs::create_dir_all(&out);
    let xml = out.join("results.xml");
    let log = out.join("editor.log");
    let mut r = UnityTestResult {
        tool: "unity_tests".into(),
        deterministic: true,
        platform: platform.into(),
        results_xml: xml.display().to_string(),
        editor_log: log.display().to_string(),
        ..Default::default()
    };
    let editor = match find_editor(cfg, project) {
        Ok(e) => e,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    r.editor = editor.display().to_string();
    let mut cmd = tokio::process::Command::new(&editor);
    cmd.args(["-batchmode", "-nographics", "-projectPath"])
        .arg(project)
        .args(["-runTests", "-testPlatform", platform, "-testResults"])
        .arg(&xml)
        .arg("-logFile")
        .arg(&log)
        .kill_on_drop(true);
    if let Some(f) = filter {
        cmd.args(["-testFilter", f]);
    }
    crate::events::emit("progress", serde_json::json!({"tool": "unity_tests", "event": "editor_started", "platform": platform}));
    let status = match tokio::time::timeout(std::time::Duration::from_secs(timeout_s), cmd.status()).await {
        Err(_) => {
            r.error = Some(format!("Unity test run timed out after {timeout_s}s"));
            return r;
        }
        Ok(Err(e)) => {
            r.error = Some(format!("cannot start Unity: {e}"));
            return r;
        }
        Ok(Ok(s)) => s,
    };
    r.duration_ms = t0.elapsed().as_millis() as u64;
    match std::fs::read_to_string(&xml) {
        Ok(x) => match parse_nunit(&x) {
            Ok((total, passed, failed, skipped, failures)) => {
                (r.total, r.passed, r.failed, r.skipped, r.failures) = (total, passed, failed, skipped, failures);
                r.approval = Some(failed == 0 && total > 0);
                if total == 0 {
                    r.error = Some("no tests ran (none found for this platform/filter?)".into());
                    r.approval = None;
                }
            }
            Err(e) => r.error = Some(format!("results XML unreadable: {e}")),
        },
        Err(_) => {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            r.log_tail = text.lines().rev().take(25).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            r.error = Some(format!("Unity exited ({status}) without writing test results; see log_tail (compile errors, licensing, or project already open)"));
        }
    }
    r
}

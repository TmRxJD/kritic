//! Process hygiene: what a project or a test run leaves behind. Facts from psutil (harness
//! `hygiene.py`), no model. Orphaned processes, sockets still bound after their owner exited
//! (an inherited handle), leftover files, and anything a desktop/resource run started and did
//! not end.

use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};

use crate::config::Config;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct HygieneResult {
    pub tool: String,
    pub deterministic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub approval: Option<bool>,
    pub reasons: Vec<String>,
    /// Harness audit: processes, orphans, dead_owner_sockets, leftovers, killed.
    pub report: Value,
    pub duration_ms: u64,
    pub full_result: Option<String>,
}

fn proc_label(p: &Value) -> String {
    let ports = p["ports"].as_array().filter(|a| !a.is_empty()).map(|a| format!(", listening on {}", a.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("/"))).unwrap_or_default();
    format!("PID {} {} (age {}s{ports})", p["pid"], p["name"].as_str().unwrap_or("?"), p["age_s"])
}

/// Pure: leaks a desktop/resource run left behind (`report["hygiene"]`) -> breakages.
pub fn judge_leaks(h: &Value) -> Vec<String> {
    let mut b = Vec::new();
    for p in h["processes"].as_array().into_iter().flatten() {
        b.push(format!("process started by the run was still alive after it ended (killed): {}", proc_label(p)));
    }
    for s in h["ports"].as_array().into_iter().flatten() {
        b.push(format!("port {} still listening after the run ended ({} PID {})", s["port"], s["name"].as_str().unwrap_or("?"), s["pid"]));
    }
    b
}

/// Pure: a machine audit -> reasons.
pub fn judge_audit(a: &Value) -> Vec<String> {
    let mut r = Vec::new();
    for p in a["orphans"].as_array().into_iter().flatten() {
        r.push(format!("orphaned process (its parent exited): {}", proc_label(p)));
    }
    for s in a["dead_owner_sockets"].as_array().into_iter().flatten() {
        let heirs: Vec<String> = s["inheritors"].as_array().into_iter().flatten().map(|h| format!("{} {}", h["pid"], h["name"].as_str().unwrap_or("?"))).collect();
        r.push(format!(
            "port {} is held by exited PID {}: a child inherited the listening handle ({})",
            s["port"],
            s["owner_pid"],
            if heirs.is_empty() { "holder unknown".to_string() } else { heirs.join(", ") }
        ));
    }
    for f in a["leftovers"].as_array().into_iter().flatten() {
        r.push(format!("leftover: {}", f.as_str().unwrap_or("?")));
    }
    r
}

pub async fn audit(cfg: &Config, roots: Vec<String>, ignore_names: &[String], leftover_globs: &[String], clean: bool) -> HygieneResult {
    let t0 = Instant::now();
    let mut r = HygieneResult { tool: "hygiene_critic".into(), deterministic: true, ..Default::default() };
    let input = json!({"mode": "hygiene", "roots": roots, "ignore_names": ignore_names, "leftover_globs": leftover_globs, "clean": clean});
    match crate::phase3::run_py_harness(cfg, &input, 90).await {
        Err(e) => r.error = Some(e),
        Ok(rep) if rep["ok"] != true => r.error = Some(rep["error"].as_str().unwrap_or("hygiene harness failed").to_string()),
        Ok(rep) => {
            r.reasons = judge_audit(&rep);
            // A finding cleaned up in this call still happened: the source must be fixed.
            r.approval = Some(r.reasons.is_empty());
            r.report = rep;
        }
    }
    r.duration_ms = t0.elapsed().as_millis() as u64;
    r
}

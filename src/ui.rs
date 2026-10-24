//! ui_test: runs the deterministic Playwright harness (harness/ui_test.mjs) and computes
//! approval from what it observed. No model is involved.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct Element {
    #[serde(default)]
    pub tag: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Interaction {
    pub page: String,
    pub element: Element,
    pub action: String,
    /// ok | navigated | no_effect | error | crash | skipped
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AxeViolation {
    pub id: String,
    #[serde(default)]
    pub impact: Option<String>,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub nodes: u64,
    #[serde(default)]
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PageReport {
    pub url: String,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub load_ms: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub screenshot: Option<String>,
    #[serde(default)]
    pub axe: Vec<AxeViolation>,
    #[serde(default)]
    pub axe_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FailedRequest {
    #[serde(default)]
    pub page: String,
    pub url: String,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub failure: Option<String>,
    #[serde(default)]
    pub count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Persistence {
    pub selector: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub expected: Option<Value>,
    #[serde(default)]
    pub actual: Option<Value>,
    pub persisted: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Raw harness output. `ok` is required; everything else defaults when absent.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct HarnessReport {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub browser_version: String,
    #[serde(default)]
    pub pages: Vec<PageReport>,
    #[serde(default)]
    pub interactions: Vec<Interaction>,
    #[serde(default)]
    pub console_errors: Vec<Value>,
    #[serde(default)]
    pub page_errors: Vec<Value>,
    #[serde(default)]
    pub failed_requests: Vec<FailedRequest>,
    #[serde(default)]
    pub crashes: Vec<Value>,
    #[serde(default)]
    pub dialogs: Vec<Value>,
    #[serde(default)]
    pub popups: Vec<Value>,
    #[serde(default)]
    pub persistence: Vec<Persistence>,
    #[serde(default)]
    pub storage_keys_written: Vec<String>,
    /// Results of scripted multi-step flows.
    #[serde(default)]
    pub flows: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct UiSummary {
    pub pages: usize,
    pub interactions: usize,
    pub ok: usize,
    pub navigated: usize,
    pub errors: usize,
    pub no_effect: usize,
    /// Heuristic interactions whose effect was not observed; these are coverage gaps, not proven broken controls.
    pub unverified_effects: Vec<String>,
    pub skipped: usize,
    pub axe_serious: usize,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct UiResult {
    pub tool: String,
    /// Every finding is an observation from a real browser; no model involved.
    pub deterministic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub summary: UiSummary,
    /// One line per problem, grouped by kind. Empty when approved.
    pub breakages: Vec<String>,
    pub approval: Option<bool>,
    pub out_dir: String,
    /// Trimmed to problem interactions; the complete report is in `full_result`.
    pub report: HarnessReport,
    pub duration_ms: u64,
    /// Path of the untrimmed JSON result.
    pub full_result: Option<String>,
}

pub struct UiOptions {
    pub axe_blocking: bool,
    /// Control names (substring match) whose no_effect/error results are ignored.
    pub ignore: Vec<String>,
}

fn is_noise(url: &str) -> bool {
    url.ends_with("/favicon.ico")
}

/// Pure: turn a harness report into breakages + approval.
pub fn judge(
    report: HarnessReport,
    opts: &UiOptions,
) -> (UiSummary, Vec<String>, Option<bool>, HarnessReport) {
    let mut s = UiSummary {
        pages: report.pages.len(),
        interactions: report.interactions.len(),
        ..Default::default()
    };
    if !report.ok {
        return (s, Vec::new(), None, report);
    }
    let mut b = judge_pages(&report.pages, opts, &mut s);
    b.extend(judge_interactions(&report.interactions, opts, &mut s));
    b.extend(judge_errors(&report));
    b.extend(judge_flows(&report.flows));
    let approval = if !b.is_empty() {
        Some(false)
    } else if report.pages.is_empty() || !s.unverified_effects.is_empty() {
        None
    } else {
        Some(true)
    };
    (s, b, approval, report)
}

/// A failed flow is one breakage naming the failing step (web and desktop share the shape).
pub fn judge_flows(flows: &[Value]) -> Vec<String> {
    flows
        .iter()
        .filter(|f| {
            f["passed"] != true
                || !f["steps"].as_array().is_some_and(|steps| {
                    !steps.is_empty() && steps.iter().all(|s| s["result"] == "ok")
                })
        })
        .map(|f| {
            let step = f["steps"]
                .as_array()
                .and_then(|s| s.iter().find(|x| x["result"] == "fail"));
            match step {
                Some(st) => format!(
                    "flow \"{}\" failed at step {} ({} {}): {}",
                    f["name"].as_str().unwrap_or("?"),
                    st["i"],
                    st["do"].as_str().unwrap_or("?"),
                    st["target"],
                    st["error"].as_str().unwrap_or("?")
                ),
                None => format!("flow \"{}\" failed", f["name"].as_str().unwrap_or("?")),
            }
        })
        .collect()
}

/// Accept object flows and JSON-encoded MCP flow strings; reject vacuous scripts before launch.
pub fn normalize_flows(flows: &Value) -> Result<Vec<Value>, String> {
    if flows.is_null() {
        return Ok(Vec::new());
    }
    flows
        .as_array()
        .ok_or("flows must be an array")?
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            let invalid = |reason: &str| format!("flow {}: {reason}", i + 1);
            let flow = match raw.as_str() {
                Some(encoded) => serde_json::from_str(encoded)
                    .map_err(|e| invalid(&format!("invalid JSON: {e}")))?,
                None => raw.clone(),
            };
            if !flow.is_object() {
                return Err(invalid("must be an object"));
            }
            let steps = flow["steps"]
                .as_array()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| invalid("steps must be a non-empty array"))?;
            if steps.iter().any(|s| {
                !s.is_object() || !s["do"].as_str().is_some_and(|action| !action.is_empty())
            }) {
                return Err(invalid(
                    "each step must be an object with a non-empty do action",
                ));
            }
            Ok(flow)
        })
        .collect()
}

fn judge_pages(pages: &[PageReport], opts: &UiOptions, s: &mut UiSummary) -> Vec<String> {
    let mut b = Vec::new();
    for p in pages {
        if let Some(e) = &p.error {
            b.push(format!("page failed to load: {} ({e})", p.url));
        } else if p.status.is_some_and(|st| st >= 400) {
            b.push(format!(
                "page returned HTTP {}: {}",
                p.status.unwrap_or(0),
                p.url
            ));
        }
        for v in p
            .axe
            .iter()
            .filter(|v| matches!(v.impact.as_deref(), Some("serious" | "critical")))
        {
            s.axe_serious += 1;
            if opts.axe_blocking {
                b.push(format!(
                    "a11y {} ({}): {} [{} nodes] on {}",
                    v.id,
                    v.impact.as_deref().unwrap_or("?"),
                    v.help,
                    v.nodes,
                    p.url
                ));
            }
        }
    }
    b
}

fn judge_interactions(
    interactions: &[Interaction],
    opts: &UiOptions,
    s: &mut UiSummary,
) -> Vec<String> {
    let ignored = |name: &str| {
        opts.ignore
            .iter()
            .any(|i| !i.is_empty() && name.contains(i.as_str()))
    };
    let mut b = Vec::new();
    for i in interactions {
        let problem = match i.result.as_str() {
            "ok" => {
                s.ok += 1;
                None
            }
            "navigated" => {
                s.navigated += 1;
                None
            }
            "skipped" => {
                s.skipped += 1;
                None
            }
            "no_effect" => {
                s.no_effect += 1;
                if !ignored(&i.element.name) {
                    s.unverified_effects.push(format!(
                        "interaction effect unverified: {} \"{}\" on {}; check application state independently",
                        i.element.tag, i.element.name, i.page
                    ));
                }
                None
            }
            _ => {
                s.errors += 1;
                Some(format!(
                    "{} on {} \"{}\" ({}): {}",
                    i.result,
                    i.element.tag,
                    i.element.name,
                    i.action,
                    i.errors.join(" | ")
                ))
            }
        };
        if let Some(msg) = problem.filter(|_| !ignored(&i.element.name)) {
            b.push(msg);
        }
    }
    b
}

/// Page/console errors not already attributed to a control, failed requests, crashes, and
/// persistence failures.
fn judge_errors(report: &HarnessReport) -> Vec<String> {
    let attributed: std::collections::HashSet<&str> = report
        .interactions
        .iter()
        .flat_map(|i| i.errors.iter().map(String::as_str))
        .collect();
    let text_of = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).unwrap_or("?").to_string();
    let mut b: Vec<String> = report
        .page_errors
        .iter()
        .map(|e| text_of(e, "message"))
        .filter(|m| !attributed.contains(m.as_str()))
        .map(|m| format!("uncaught page error: {m}"))
        .collect();
    // 4xx/5xx resource loads are reported once, under failed requests
    b.extend(
        report
            .console_errors
            .iter()
            .map(|e| text_of(e, "text"))
            .filter(|t| {
                !t.starts_with("Failed to load resource") && !attributed.contains(t.as_str())
            })
            .map(|t| format!("console error: {t}")),
    );
    b.extend(
        report
            .failed_requests
            .iter()
            .filter(|f| !is_noise(&f.url))
            .map(|f| {
                let why = f
                    .status
                    .map(|x| format!("HTTP {x}"))
                    .or_else(|| f.failure.clone())
                    .unwrap_or_default();
                format!("request failed: {} ({why})", f.url)
            }),
    );
    b.extend(
        report
            .crashes
            .iter()
            .map(|c| format!("page crashed: {}", text_of(c, "page"))),
    );
    b.extend(report.persistence.iter().filter(|p| !p.persisted).map(|p| {
        let err = p
            .error
            .as_deref()
            .map(|e| format!(", {e}"))
            .unwrap_or_default();
        format!(
            "value did not persist across reload: {} (expected {:?}, got {:?}{err})",
            p.selector, p.expected, p.actual
        )
    }));
    b
}

/// Run the harness with `input` on stdin; progress streams to the event bus; parse stdout.
pub async fn run_harness(cfg: &Config, input: &Value) -> Result<HarnessReport, String> {
    let v = tokio::time::timeout(
        std::time::Duration::from_secs(cfg.ui_timeout_s),
        crate::native::web::ui_test(cfg, input),
    )
    .await
    .map_err(|_| format!("ui_test timed out after {}s", cfg.ui_timeout_s))?;
    serde_json::from_value::<HarnessReport>(v).map_err(|e| format!("ui_test report not valid: {e}"))
}

#[allow(clippy::too_many_arguments)]
pub async fn ui_test(
    cfg: &Config,
    url: &str,
    routes: &[String],
    persist: &[Value],
    max_pages: usize,
    max_actions: usize,
    allow_destructive: bool,
    allow_remote: bool,
    headed: bool,
    axe: bool,
    flows: &[Value],
    explore: bool,
    opts: UiOptions,
) -> UiResult {
    let out_dir = crate::vision::run_dir(cfg, "ui");
    let input = json!({
        "mode": "ui_test", "url": url, "routes": routes, "persist": persist,
        "max_pages": max_pages, "max_actions": max_actions, "allow_destructive": allow_destructive,
        "allow_remote": allow_remote, "headed": headed, "axe": axe, "out_dir": out_dir,
        "flows": flows, "explore": explore,
    });
    let t0 = Instant::now();
    let mut r = UiResult {
        tool: "ui_test".into(),
        deterministic: true,
        out_dir: out_dir.display().to_string(),
        ..Default::default()
    };
    match run_harness(cfg, &input).await {
        Err(e) => r.error = Some(e),
        Ok(report) => {
            if !report.ok {
                r.error = report.error.clone();
            }
            let (summary, breakages, approval, report) = judge(report, &opts);
            r.summary = summary;
            r.breakages = breakages;
            r.approval = approval;
            r.report = report;
        }
    }
    r.duration_ms = t0.elapsed().as_millis() as u64;
    r
}

/// Screenshot a URL (for visual_critic).
pub async fn screenshot(
    cfg: &Config,
    url: &str,
    out_path: &std::path::Path,
    full_page: bool,
    allow_remote: bool,
) -> Result<(), String> {
    let v = tokio::time::timeout(
        std::time::Duration::from_secs(90),
        crate::native::web::screenshot(cfg, url, out_path, full_page, allow_remote),
    )
    .await
    .map_err(|_| "screenshot timed out after 90s".to_string())?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(v
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("screenshot failed")
            .to_string());
    }
    Ok(())
}

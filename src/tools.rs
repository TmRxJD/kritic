//! Tool logic independent of MCP plumbing: route, context_trim, artifact_validate, status, log.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};

use crate::checks::{Check, CommandResult, Status, deterministic_checks, run_command};
use crate::config::Config;
use crate::critic::{CriticResult, critic};
use crate::files::{Skipped, kind_for_path, read_lossy, resolve_paths};
use crate::kev::{Decider, KevClient};

// ------------------------------------------------------------------ route

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct RouteResult {
    pub tool: String,
    pub advisory: bool,
    pub kev_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub suggestion: Option<String>,
    pub confidence: Option<f64>,
    pub probabilities: Option<BTreeMap<String, f64>>,
    /// Fixed text describing the chosen tier (Kev does not write reasons).
    pub basis: Option<String>,
    pub note: String,
    /// Exact coding model for Codex routing; classification backends are never executors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coding_model: Option<String>,
}

pub async fn route<D: Decider>(d: &D, task: &str, hints: &str, artifact_type: &str) -> RouteResult {
    route_spec(d, task, hints, artifact_type, "route").await
}

/// Conservative Codex-only tier selection; low-confidence or unavailable advice stays on Sol.
pub async fn route_codex<D: Decider>(
    d: &D,
    task: &str,
    hints: &str,
    artifact_type: &str,
) -> RouteResult {
    let mut result = route_spec(d, task, hints, artifact_type, "route_codex").await;
    result.coding_model = Some(codex_model(result.suggestion.as_deref(), result.confidence).into());
    result.note = "Codex performs all work: Sol standard, Astra hard, Luna basic. Kev/JEV only classifies. Uncertain/unavailable routing stays on Sol; recommendations are not calibrated gates.".into();
    result
}

pub fn codex_model(tier: Option<&str>, confidence: Option<f64>) -> &'static str {
    if !confidence.is_some_and(|p| p.is_finite() && (0.75..=1.0).contains(&p)) {
        return "gpt-6.1-sol";
    }
    match tier {
        Some("luna") => "gpt-6-luna",
        Some("astra") => "gpt-6-astra",
        _ => "gpt-6.1-sol",
    }
}

async fn route_spec<D: Decider>(
    d: &D,
    task: &str,
    hints: &str,
    artifact_type: &str,
    name: &str,
) -> RouteResult {
    let mut r = RouteResult {
        tool: "route".into(),
        advisory: true,
        note: "Suggestion only; the user's Sonnet/Haiku/Opus routing rules take precedence.".into(),
        ..Default::default()
    };
    if task.trim().is_empty() {
        r.error = Some("empty task".into());
        return r;
    }
    let spec = crate::spec::get(name);
    let mut described = task.to_string();
    if !hints.trim().is_empty() {
        described.push_str(&format!("\nHints: {hints}"));
    }
    if !artifact_type.trim().is_empty() {
        described.push_str(&format!("\nProduces: {artifact_type}"));
    }
    let state = match spec.state(&[("task", described)]) {
        Ok(s) => s,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    match d.decide(&state, &spec.wire()).await {
        Err(e) => r.error = Some(e),
        Ok(dec) => {
            let Some(a) = dec.answers.get("tier") else {
                r.error = Some("routing decision is missing the tier answer".into());
                return r;
            };
            if dec.truncated || !a.value.is_finite() || !(0.0..=1.0).contains(&a.value) {
                r.error = Some("routing decision is truncated or has an invalid confidence".into());
                return r;
            }
            r.kev_ok = true;
            r.suggestion = a.choice.clone();
            r.confidence = Some(a.value);
            r.probabilities = a.probabilities.clone();
            r.basis = match (a.choice.as_deref(), spec.question("tier").map(|q| &q.ask)) {
                (Some(c), Some(crate::spec::Ask::Choice { options, .. })) => {
                    options.iter().find(|(k, _)| k == c).map(|(_, d)| d.clone())
                }
                _ => None,
            };
        }
    }
    r
}

// ------------------------------------------------------------------ context_trim

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Ranked {
    pub index: usize,
    pub relevance: f64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct TrimResult {
    pub tool: String,
    pub advisory: bool,
    pub kev_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Most relevant first.
    pub ranked: Vec<Ranked>,
    pub kept: Vec<usize>,
    pub dropped: Vec<usize>,
    pub chars_kept: usize,
    pub trimmed_context: String,
    /// Kev cannot write summaries; chunks are kept or dropped whole.
    pub note: String,
}

pub async fn context_trim<D: Decider>(
    d: &D,
    cfg: &Config,
    query: &str,
    chunks: &[String],
    budget_chars: usize,
    min_relevance: f64,
    top_n: Option<usize>,
) -> TrimResult {
    let mut r = TrimResult {
        tool: "context_trim".into(),
        advisory: true,
        note: "Extractive only: chunks are kept or dropped whole; no summaries are generated."
            .into(),
        ..Default::default()
    };
    if chunks.len() > 64 {
        r.error = Some("max 64 chunks per call; batch them".into());
        return r;
    }
    let spec = crate::spec::get("context_relevance");
    let q = spec.wire();
    let mut scores = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        // Relevance is judged on the chunk's opening; the whole chunk is kept or dropped.
        let clipped: String = c.chars().take(cfg.chunk_chars.min(1400)).collect();
        let state = match spec.state(&[("query", query.to_string()), ("chunk", clipped)]) {
            Ok(s) => s,
            Err(e) => {
                r.error = Some(format!("{e} (chunk {i})"));
                return r;
            }
        };
        match d.decide(&state, &q).await {
            Err(e) => {
                r.error = Some(format!("{e} (chunk {i})"));
                return r;
            }
            Ok(dec) => scores.push(dec.answers["relevant"].value),
        }
    }
    r.kev_ok = true;
    let mut order: Vec<usize> = (0..chunks.len()).collect();
    order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    r.ranked = order
        .iter()
        .map(|&i| Ranked {
            index: i,
            relevance: (scores[i] * 1000.0).round() / 1000.0,
        })
        .collect();
    let mut kept = Vec::new();
    let mut used = 0;
    for &i in &order {
        if scores[i] < min_relevance
            || used + chunks[i].len() > budget_chars
            || top_n.is_some_and(|n| kept.len() >= n)
        {
            continue;
        }
        kept.push(i);
        used += chunks[i].len();
    }
    kept.sort_unstable();
    r.dropped = (0..chunks.len()).filter(|i| !kept.contains(i)).collect();
    r.trimmed_context = kept
        .iter()
        .map(|&i| chunks[i].as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    r.chars_kept = used;
    r.kept = kept;
    r
}

// ------------------------------------------------------------------ files -> critic

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct FileCheck {
    pub path: String,
    pub kind: String,
    pub checks: Vec<Check>,
}

pub struct LoadedFiles {
    pub files: Vec<(String, String)>,
    pub checks: Vec<FileCheck>,
    pub skipped: Vec<Skipped>,
    pub missing: Vec<String>,
}

pub async fn load_files(paths: &[String], base_dir: Option<&str>) -> LoadedFiles {
    let (usable, skipped) = resolve_paths(paths, base_dir);
    let mut files = Vec::new();
    let mut checks = Vec::new();
    for p in usable {
        let kind = kind_for_path(&p);
        match read_lossy(&p) {
            Ok(text) => {
                checks.push(FileCheck {
                    path: p.display().to_string(),
                    kind: kind.into(),
                    checks: deterministic_checks(&text, kind).await,
                });
                files.push((p.display().to_string(), text));
            }
            Err(e) => checks.push(FileCheck {
                path: p.display().to_string(),
                kind: kind.into(),
                checks: vec![Check {
                    check: "read".into(),
                    status: Status::Fail,
                    detail: Some(e.to_string()),
                }],
            }),
        }
    }
    let missing = skipped
        .iter()
        .filter(|s| s.reason == "not found")
        .map(|s| s.path.clone())
        .collect();
    LoadedFiles {
        files,
        checks,
        skipped,
        missing,
    }
}

// ------------------------------------------------------------------ git diff

async fn git(base: &std::path::Path, args: &[&str], paths: &[String]) -> Result<String, String> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .arg("--")
        .args(paths)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("git not available: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Uncommitted changes vs HEAD for `paths` (all when empty), plus untracked files rendered as
/// new-file diffs. Untracked files go through the same filters as `paths` (binaries, vendored
/// dirs, size limit).
pub async fn git_diff(base_dir: Option<&str>, paths: &[String]) -> Result<String, String> {
    let base = base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let mut diff = git(
        &base,
        &["diff", "HEAD", "--no-color", "--no-ext-diff", "-U3"],
        paths,
    )
    .await?;
    let untracked = git(
        &base,
        &["ls-files", "--others", "--exclude-standard"],
        paths,
    )
    .await?;
    let names: Vec<String> = untracked
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();
    let (usable, _) = resolve_paths(&names, base.to_str());
    // resolve_paths canonicalizes, so strip the canonical base: a short-name base
    // (C:\Users\RUNNER~1\...) never prefixes the long-name paths of the files under it.
    let canon_base = crate::files::dunce_canonical(&base);
    for p in usable {
        let Ok(text) = read_lossy(&p) else { continue };
        let rel = p
            .strip_prefix(&canon_base)
            .or_else(|_| p.strip_prefix(&base))
            .unwrap_or(&p)
            .display()
            .to_string()
            .replace('\\', "/");
        let n = text.lines().count();
        diff.push_str(&format!(
            "diff --git a/{rel} b/{rel}\nnew file mode 100644\n@@ -0,0 +1,{n} @@\n"
        ));
        for l in text.lines() {
            diff.push('+');
            diff.push_str(l);
            diff.push('\n');
        }
    }
    Ok(diff)
}

// ------------------------------------------------------------------ artifact_validate

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct ValidateResult {
    pub tool: String,
    pub advisory: bool,
    pub files: Vec<FileCheck>,
    pub skipped: Vec<Skipped>,
    pub missing_files: Vec<String>,
    /// The project's own validators, run exactly as given, output verbatim.
    pub commands: Vec<CommandResult>,
    pub deterministic_ok: bool,
    pub critic: Option<CriticResult>,
    /// false if any deterministic check or command failed; else the critic's approval
    /// (null when the critic was skipped-offline or coverage was incomplete).
    pub approval: Option<bool>,
    pub reasons: Vec<String>,
}

pub struct ValidateInput<'a> {
    pub content: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub paths: &'a [String],
    pub base_dir: Option<&'a str>,
    pub commands: &'a [String],
    pub task: &'a str,
    pub use_model: bool,
    pub revision: u32,
}

pub async fn artifact_validate<D: Decider>(
    d: &D,
    cfg: &Config,
    inp: ValidateInput<'_>,
) -> ValidateResult {
    let mut r = ValidateResult {
        tool: "artifact_validate".into(),
        advisory: true,
        ..Default::default()
    };
    let mut files = Vec::new();
    if !inp.paths.is_empty() {
        let lf = load_files(inp.paths, inp.base_dir).await;
        r.files = lf.checks;
        r.skipped = lf.skipped;
        r.missing_files = lf.missing;
        files = lf.files;
    } else if let Some(c) = inp.content {
        let kind = inp.kind.unwrap_or("text");
        r.files.push(FileCheck {
            path: "<content>".into(),
            kind: kind.into(),
            checks: deterministic_checks(c, kind).await,
        });
        files.push(("<content>".into(), c.to_string()));
    } else if inp.commands.is_empty() {
        r.approval = Some(false);
        r.reasons.push("give paths, content, or commands".into());
        return r;
    }
    let dir = inp
        .base_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    for cmd in inp.commands {
        r.commands
            .push(run_command(cmd, &dir, cfg.command_timeout_s).await);
    }
    for f in &r.files {
        for c in f.checks.iter().filter(|c| c.status == Status::Fail) {
            r.reasons.push(format!(
                "{}: {} failed{}",
                f.path,
                c.check,
                c.detail
                    .as_deref()
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default()
            ));
        }
    }
    for m in &r.missing_files {
        r.reasons.push(format!("missing file: {m}"));
    }
    for c in r.commands.iter().filter(|c| c.status == Status::Fail) {
        r.reasons.push(format!(
            "command failed (exit {:?}): {}",
            c.exit_code, c.command
        ));
    }
    r.deterministic_ok = r.reasons.is_empty();
    let mut gaps = Vec::new();
    if r.files.is_empty() && r.commands.is_empty() {
        gaps.push("no artifacts or commands were checked".to_string());
    }
    if !r.skipped.is_empty() {
        gaps.push(format!(
            "{} artifacts skipped; coverage is incomplete",
            r.skipped.len()
        ));
    }
    if files.iter().any(|(_, content)| content.trim().is_empty()) {
        gaps.push("empty artifact content does not establish acceptance".into());
    }
    if r.files
        .iter()
        .flat_map(|f| &f.checks)
        .any(|c| c.status == Status::Skipped)
        || r.commands.iter().any(|c| c.status == Status::Skipped)
    {
        gaps.push("requested checks were skipped".into());
    }
    if inp.use_model && files.is_empty() {
        gaps.push("model review requested but no reviewable artifacts".into());
    }

    if inp.use_model && !files.is_empty() {
        let c = critic(d, cfg, inp.task, &files, inp.revision).await;
        r.critic = Some(c);
    }
    r.approval = if !r.deterministic_ok {
        Some(false)
    } else {
        match &r.critic {
            None => Some(true),
            Some(c) => {
                if !c.kev_ok {
                    r.reasons.push(format!(
                        "critic offline: {}",
                        c.error.as_deref().unwrap_or("?")
                    ));
                }
                r.reasons.extend(c.reasons.iter().cloned());
                c.approval
            }
        }
    };
    if !gaps.is_empty() {
        if r.approval == Some(true) {
            r.approval = None;
        }
        r.reasons.extend(gaps);
    }
    r
}

// ------------------------------------------------------------------ status

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct StatusResult {
    pub tool: String,
    pub capabilities: BTreeMap<String, bool>,
    pub enabled_tools: Vec<String>,
    pub cloud_provider: String,
    /// Configuration is not a paid connectivity test. Use doctor --online explicitly.
    pub cloud_verification: String,
    pub vision_backend: String,
    /// `kev.backend` as configured (auto, local, cloud, off).
    pub backend_setting: String,
    /// The backend Kev questions go to (local or cloud), or null when none is usable.
    pub backend: Option<String>,
    /// Why no backend is usable, with what to do about it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_error: Option<String>,
    /// The model answering Kev questions (kev-4b locally, typesafe-ai/jev in the cloud).
    pub decision_model: Option<String>,
    /// Whether the built-in gates were calibrated for that model; when false, Kev results are
    /// reported but carry no verdict.
    pub gates_calibrated: bool,
    /// A gateway key is configured (the key itself is never shown).
    pub cloud_key_configured: bool,
    pub laya_up: bool,
    pub laya_url: String,
    pub health: Option<Value>,
    pub laya_exe: String,
    pub laya_exe_exists: bool,
    pub kev_gguf: String,
    pub kev_gguf_exists: bool,
    pub model: String,
    pub chunk_chars: usize,
    pub token_budget: usize,
    /// Every calibrated Kev gate (spec.question: rule). Only these can fail approval; each spec
    /// file holds the calibration evidence. Everything else Kev answers is advisory.
    pub gates: Vec<String>,
    /// Built-in spec lint errors (always empty in a correct build).
    pub spec_errors: Vec<String>,
    pub max_revisions: u32,
    pub log: String,
    /// The kritic.toml that was read, if any.
    pub config_file: Option<String>,
    /// Problems in that file (bad TOML, unknown keys); their settings fell back to defaults.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub config_errors: Vec<String>,
    /// Opt-in project setup guidance; no extra payload on ordinary status calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_setup: Option<crate::setup_guidance::ProjectSetup>,
    /// Live event stream (ready states, tool lifecycle, harness progress). Subscribe with
    /// Claude Code's Monitor tool: ws source.
    pub ws_url: Option<String>,
    /// Current ready states of laya / vision / gpu.
    pub ready: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_error: Option<String>,
}

pub async fn status(k: &KevClient, start: bool) -> StatusResult {
    let cfg = &k.cfg;
    let resolved = k.resolve_backend().await;
    let start_error = match (&resolved, start) {
        (Ok(crate::kev::Backend::Local), true) => k.ensure_up().await.err(),
        _ => None,
    };
    let health = if matches!(resolved, Ok(crate::kev::Backend::Local)) {
        k.health().await
    } else {
        None
    };
    let decision_model = resolved.as_ref().ok().map(|b| k.model_for(b).to_string());
    StatusResult {
        tool: "status".into(),
        capabilities: cfg.capabilities.clone(),
        enabled_tools: crate::config::TOOL_NAMES
            .iter()
            .filter(|n| cfg.tool_enabled(n))
            .map(|n| n.to_string())
            .collect(),
        cloud_provider: cfg.cloud_provider.clone(),
        cloud_verification: "not_probed (doctor --online makes an API call)".into(),
        vision_backend: cfg.vision_backend.clone(),
        backend_setting: cfg.backend.clone(),
        backend: resolved.as_ref().ok().map(|b| b.label().to_string()),
        backend_error: resolved.as_ref().err().cloned(),
        gates_calibrated: resolved.is_ok()
            && crate::spec::builtin().values().all(|s| {
                s.gates_apply_for(
                    &k.model_identity()
                        .unwrap_or_else(|| cfg.decision_identity()),
                )
            }),
        decision_model,
        cloud_key_configured: cfg.cloud_key().is_some(),
        laya_up: health.is_some(),
        laya_url: cfg.laya_url.clone(),
        health,
        laya_exe: cfg.laya_exe.display().to_string(),
        laya_exe_exists: cfg.laya_exe.exists(),
        kev_gguf: cfg.kev_gguf.display().to_string(),
        kev_gguf_exists: cfg.kev_gguf.exists(),
        model: cfg.model.clone(),
        chunk_chars: cfg.chunk_chars,
        token_budget: cfg.token_budget,
        gates: crate::spec::builtin()
            .values()
            .flat_map(|spec| {
                spec.questions.iter().filter_map(move |q| match &q.gate {
                    crate::spec::Gate::Rule {
                        measure,
                        fails_when,
                        threshold,
                        ..
                    } => Some(
                        format!(
                            "{}.{}: fails when {:?} is {:?} {threshold}",
                            spec.name, q.id, measure, fails_when
                        )
                        .to_lowercase(),
                    ),
                    crate::spec::Gate::Advisory { .. } => None,
                })
            })
            .collect(),
        spec_errors: crate::spec::lint_builtin(),
        max_revisions: cfg.max_revisions,
        log: cfg.log_path.display().to_string(),
        config_file: cfg.config_file.as_ref().map(|p| p.display().to_string()),
        config_errors: cfg.config_errors.clone(),
        project_setup: None,
        ws_url: crate::events::url(),
        ready: crate::events::snapshot()["ready"].clone(),
        start_error,
    }
}

// ------------------------------------------------------------------ output compaction

/// Write the complete result to `<run dir>/result.json` and return its path. Tool responses stay
/// small so they don't bloat the caller's context; the full data is one file read away.
pub fn save_full(cfg: &Config, tool: &str, result: &impl Serialize) -> Option<String> {
    let dir = crate::vision::run_dir(cfg, tool);
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("result.json");
    std::fs::write(&path, serde_json::to_vec_pretty(result).ok()?).ok()?;
    Some(path.display().to_string())
}

pub const MAX_ITEMS: usize = 25;

// ------------------------------------------------------------------ audit log

/// Append one JSON line per call. Never fails the tool.
pub fn audit(cfg: &Config, tool: &str, result: &impl Serialize) {
    let v = serde_json::to_value(result).unwrap_or(Value::Null);
    let pick = |k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    let rec = json!({
        "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "tool": tool,
        "kev_ok": pick("kev_ok"),
        "approval": pick("approval"),
        "error": pick("error"),
        "correctness": pick("correctness"),
        "completeness": pick("completeness"),
        "quality": pick("quality"),
        "suggestion": pick("suggestion"),
        "latency_ms": pick("latency_ms"),
    });
    let path = &cfg.log_path;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if path
        .metadata()
        .map(|m| m.len() > 2_000_000)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(path, path.with_extension("old.jsonl"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{rec}");
    }
}

/// Source files under `base` (for whole-project quality scans), skipping vendored/build dirs.
pub fn source_files(base: &std::path::Path, limit: usize) -> Vec<String> {
    const EXT: &[&str] = &[
        "rs", "py", "ts", "tsx", "js", "jsx", "mjs", "cjs", "go", "cs", "java", "kt", "swift", "c",
        "cpp", "h", "gleam", "svelte", "vue",
    ];
    const SKIP: &[&str] = &[
        "node_modules",
        "target",
        "dist",
        "build",
        ".git",
        ".venv",
        "venv",
        "__pycache__",
        ".next",
        "coverage",
        "graphify-out",
    ];
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let n = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !n.starts_with('.') && !SKIP.contains(&n.as_str()) {
                    stack.push(p);
                }
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| EXT.contains(&x))
            {
                out.push(p.display().to_string());
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

/// A WebSocket URL restricted to loopback; plaintext transport never reaches a remote host.
pub fn require_loopback_ws(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "app_events URL is invalid".to_string())?;
    if !matches!(parsed.scheme(), "ws" | "wss") {
        return Err("app_events must use a WebSocket scheme".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("app_events must not carry credentials: {url}"));
    }
    match parsed
        .host_str()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "127.0.0.1" | "localhost" | "[::1]" => Ok(()),
        _ => Err(format!(
            "refusing non-local app_events {url} (send steps would push commands to it); pass allow_remote_app_events for your own remote app"
        )),
    }
}

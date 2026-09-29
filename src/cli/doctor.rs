//! `kritic doctor`: check every feature and say exactly what to do about each problem.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;
use serde::Serialize;
use serde_json::{Value, json};

use super::{detect, register, ui};
use crate::config::Config;
use crate::kev::{Backend, KevClient};

#[derive(Args, Debug, Default)]
pub struct DoctorArgs {
    /// Also start local Kev (or call cloud Jev) and time one real question
    #[arg(long)]
    pub online: bool,
    /// Print the checks as JSON on stdout
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Fail,
    Skip,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub area: &'static str,
    pub name: String,
    pub level: Level,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

fn check(
    area: &'static str,
    name: &str,
    level: Level,
    detail: impl Into<String>,
    fix: Option<&str>,
) -> Check {
    Check {
        area,
        name: name.to_string(),
        level,
        detail: detail.into(),
        fix: fix.map(String::from),
    }
}

pub fn exe_path() -> PathBuf {
    std::env::current_exe()
        .map(|p| crate::files::dunce_canonical(&p))
        .unwrap_or_else(|_| PathBuf::from("kritic"))
}

pub async fn checks(cfg: Arc<Config>, online: bool) -> Vec<Check> {
    let mut c = Vec::new();
    // ---- config
    match &cfg.config_file {
        Some(p) => c.push(check(
            "config",
            "kritic.toml",
            Level::Ok,
            p.display().to_string(),
            None,
        )),
        None => c.push(check(
            "config",
            "kritic.toml",
            Level::Ok,
            format!("none; using defaults under {}", cfg.home.display()),
            None,
        )),
    }
    for e in &cfg.config_errors {
        c.push(check(
            "config",
            "kritic.toml",
            Level::Fail,
            e.clone(),
            Some("fix or remove that line (see examples/kritic.toml)"),
        ));
    }
    // ---- Kev
    let kev = KevClient::new(cfg.clone());
    match kev.resolve_backend().await {
        Err(e) if cfg.backend == "off" => c.push(check("kev", "backend", Level::Skip, e, None)),
        Err(e) => c.push(check(
            "kev",
            "backend",
            Level::Fail,
            e,
            Some("kritic setup"),
        )),
        Ok(Backend::Local) => {
            c.push(check(
                "kev",
                "backend",
                Level::Ok,
                format!("local Kev ({})", cfg.model),
                None,
            ));
            let gguf = cfg.kev_gguf.metadata().map(|m| m.len()).unwrap_or(0);
            c.push(if gguf > 1_000_000_000 {
                check(
                    "kev",
                    "weights",
                    Level::Ok,
                    format!("{} ({})", cfg.kev_gguf.display(), ui::bytes(gguf)),
                    None,
                )
            } else {
                check(
                    "kev",
                    "weights",
                    Level::Fail,
                    format!("{} missing or incomplete", cfg.kev_gguf.display()),
                    Some("kritic setup"),
                )
            });
            if cfg!(windows) && cfg.laya_exe.is_file() {
                let dir = cfg.laya_exe.parent().unwrap_or(Path::new("."));
                let cuda_build = std::fs::metadata(&cfg.laya_exe)
                    .map(|m| m.len() > 100_000_000)
                    .unwrap_or(false);
                if cuda_build
                    && !dir.join("cublas64_12.dll").is_file()
                    && crate::config::which("cublas64_12").is_none()
                {
                    c.push(check(
                        "kev",
                        "CUDA runtime",
                        Level::Fail,
                        format!("cublas64_12.dll not beside {}", cfg.laya_exe.display()),
                        Some("kritic setup (fetches NVIDIA's CUDA 12 runtime DLLs)"),
                    ));
                }
            }
            match kev.health().await {
                Some(_) => c.push(check(
                    "kev",
                    "laya",
                    Level::Ok,
                    format!("answering at {}", cfg.laya_url),
                    None,
                )),
                None => c.push(check(
                    "kev",
                    "laya",
                    Level::Ok,
                    "not running; kritic starts it on first use (~40 s the very first time)",
                    None,
                )),
            }
        }
        Ok(Backend::Cloud { .. }) => {
            c.push(check(
                "kev",
                "backend",
                Level::Ok,
                format!("cloud Jev ({}, key configured)", cfg.cloud_model),
                None,
            ));
            c.push(check(
                "kev",
                "gates",
                Level::Warn,
                "Kev's calibrated gates are advisory under Jev: critic_score reports probabilities, not a verdict",
                Some("run local Kev (kritic setup --backend local), or calibrate Jev (kritic calibrate)"),
            ));
        }
    }
    if online && cfg.backend != "off" {
        c.push(online_probe(&kev).await);
    }
    // ---- vision
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .expect("http");
    let tags = http
        .get(format!("{}/api/tags", cfg.ollama_url))
        .send()
        .await
        .ok();
    let tags: Option<Value> = match tags {
        Some(r) => r.json().await.ok(),
        None => None,
    };
    match tags {
        Some(v) => {
            let has = v["models"].as_array().is_some_and(|m| {
                m.iter().any(|x| {
                    x["name"]
                        .as_str()
                        .is_some_and(|n| n.starts_with(&cfg.vision_model))
                })
            });
            c.push(if has {
                check(
                    "vision",
                    &cfg.vision_model,
                    Level::Ok,
                    "pulled; Ollama answering",
                    None,
                )
            } else {
                check(
                    "vision",
                    &cfg.vision_model,
                    Level::Warn,
                    "Ollama is running but the model is not pulled",
                    Some(&format!("ollama pull {}", cfg.vision_model)),
                )
            });
        }
        None if cfg.ollama_exe.is_file() => c.push(check(
            "vision",
            "Ollama",
            Level::Ok,
            format!(
                "installed ({}); started on demand",
                cfg.ollama_exe.display()
            ),
            None,
        )),
        None => c.push(check(
            "vision",
            "Ollama",
            Level::Warn,
            "not installed: visual_critic and the ux_critic vision novice are unavailable",
            Some("install Ollama (https://ollama.com), then kritic setup"),
        )),
    }
    // ---- web testing (Chrome/Edge over CDP)
    match crate::native::web::find_browser(&cfg) {
        Ok(b) => c.push(check(
            "web",
            "browser",
            Level::Ok,
            b.display().to_string(),
            None,
        )),
        Err(e) => c.push(check(
            "web",
            "browser",
            Level::Warn,
            format!("{e}: ui_test, visual_critic (url) and web ux_critic are unavailable"),
            Some("kritic setup (downloads Chrome for Testing)"),
        )),
    }
    // ---- desktop harness
    if cfg!(windows) {
        match detect::tool(&cfg.uv_exe, &[]) {
            None => c.push(check(
                "desktop",
                "uv",
                Level::Warn,
                "uv not found: desktop_test, unity_tests and desktop ux_critic are unavailable",
                Some("install uv (https://docs.astral.sh/uv/), then kritic setup"),
            )),
            Some(_) if !cfg.harness_py.join(".venv").is_dir() => c.push(check(
                "desktop",
                "harness",
                Level::Warn,
                "Python environment not created yet (made on first use)",
                Some("kritic setup"),
            )),
            Some(_) => c.push(check(
                "desktop",
                "harness",
                Level::Ok,
                cfg.harness_py.display().to_string(),
                None,
            )),
        }
    } else {
        c.push(check(
            "desktop",
            "desktop_test",
            Level::Skip,
            "Windows only (UI Automation)",
            None,
        ));
    }
    // ---- scanners
    for (name, bin) in [
        ("gitleaks", &cfg.tools_bin),
        ("osv-scanner", &cfg.tools_bin),
        ("semgrep", &cfg.tools_bin),
        ("bandit", &cfg.tools_bin),
        ("ruff", &cfg.tools_bin),
    ] {
        match crate::scan::tool_path(bin, name) {
            Some(p) => c.push(check(
                "security",
                name,
                Level::Ok,
                p.display().to_string(),
                None,
            )),
            None => c.push(check(
                "security",
                name,
                Level::Warn,
                "not installed: security_critic reports it as failed (approval null)",
                Some("kritic setup"),
            )),
        }
    }
    if crate::scan::tool_path(&cfg.cargo_tools_bin, "cargo-audit").is_some() {
        c.push(check(
            "security",
            "cargo-audit",
            Level::Ok,
            "installed (Rust cross-check)",
            None,
        ));
    } else {
        c.push(check(
            "security",
            "cargo-audit",
            Level::Skip,
            "optional Rust cross-check; osv-scanner covers Cargo.lock",
            Some("cargo install cargo-audit --locked"),
        ));
    }
    // ---- unity
    let editors: Vec<String> = std::fs::read_dir(&cfg.unity_hub_editors)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    if editors.is_empty() {
        c.push(check(
            "unity",
            "editors",
            Level::Skip,
            "no Unity Hub editors (only needed for unity_tests)",
            None,
        ));
    } else {
        c.push(check(
            "unity",
            "editors",
            Level::Ok,
            editors.join(", "),
            None,
        ));
    }
    // ---- agents
    let me = exe_path();
    if register::claude_present() {
        let entry = std::fs::read_to_string(register::claude_json())
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| {
                v.pointer("/mcpServers/kritic/command")
                    .and_then(Value::as_str)
                    .map(String::from)
            });
        c.push(match entry {
            None => check(
                "agents",
                "Claude Code",
                Level::Warn,
                "kritic is not registered",
                Some("kritic register --claude"),
            ),
            Some(cmd) if same_file(Path::new(&cmd), &me) => check(
                "agents",
                "Claude Code",
                Level::Ok,
                format!("registered ({cmd})"),
                None,
            ),
            Some(cmd) => check(
                "agents",
                "Claude Code",
                Level::Warn,
                format!("registered, but runs {cmd}, not this binary"),
                Some("kritic register --claude"),
            ),
        });
        let skill = register::claude_dir()
            .join("skills")
            .join("kritic")
            .join("SKILL.md");
        if !skill.is_file() {
            c.push(check(
                "agents",
                "Claude Code skill",
                Level::Warn,
                "the kritic skill is not installed",
                Some("kritic register --claude"),
            ));
        }
    }
    if register::hermes_present() {
        let text = std::fs::read_to_string(register::hermes_home().join("config.yaml"))
            .unwrap_or_default();
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&text).unwrap_or(serde_yaml::Value::Null);
        let has_mcp = yaml
            .get("mcp_servers")
            .and_then(|m| m.get("kritic"))
            .is_some();
        let engine = yaml
            .get("context")
            .and_then(|c| c.get("engine"))
            .and_then(|e| e.as_str())
            .unwrap_or("compressor")
            .to_string();
        let plugin = register::hermes_home()
            .join("plugins")
            .join("kritic")
            .join("__init__.py")
            .is_file();
        c.push(if has_mcp && plugin {
            check(
                "agents",
                "Hermes",
                Level::Ok,
                format!(
                    "MCP server and context engine plugin installed; context.engine = {engine}"
                ),
                None,
            )
        } else {
            check(
                "agents",
                "Hermes",
                Level::Warn,
                "kritic is not fully registered",
                Some("kritic register --hermes"),
            )
        });
    }
    c
}

fn same_file(a: &Path, b: &Path) -> bool {
    crate::files::dunce_canonical(a)
        .to_string_lossy()
        .to_lowercase()
        .replace('\\', "/")
        == crate::files::dunce_canonical(b)
            .to_string_lossy()
            .to_lowercase()
            .replace('\\', "/")
}

/// Ask one real question and time it.
async fn online_probe(kev: &KevClient) -> Check {
    let spec = crate::spec::get("context_relevance");
    let state = match spec.state(&[
        ("query", "What does add return?".to_string()),
        (
            "chunk",
            "fn add(a: i32, b: i32) -> i32 { a + b }".to_string(),
        ),
    ]) {
        Ok(s) => s,
        Err(e) => return check("kev", "live question", Level::Fail, e, None),
    };
    let t0 = std::time::Instant::now();
    match crate::kev::Decider::decide(kev, &state, &spec.wire()).await {
        Ok(d) => check(
            "kev",
            "live question",
            Level::Ok,
            format!(
                "answered in {:.1} s (p = {:.2})",
                t0.elapsed().as_secs_f64(),
                d.answers["relevant"].value
            ),
            None,
        ),
        Err(e) => check(
            "kev",
            "live question",
            Level::Fail,
            e,
            Some(
                "kritic doctor again after fixing the item above; laya's log is in the logs folder",
            ),
        ),
    }
}

pub fn print(checks: &[Check]) {
    let mut area = "";
    for c in checks {
        if c.area != area {
            area = c.area;
            eprintln!("\n{area}");
        }
        let line = format!("{}: {}", c.name, c.detail);
        match c.level {
            Level::Ok => ui::ok(&line),
            Level::Warn => ui::warn(&line),
            Level::Fail => ui::fail(&line),
            Level::Skip => ui::skip(&line),
        }
        if let Some(f) = &c.fix
            && c.level != Level::Ok
        {
            ui::info(&format!("fix: {f}"));
        }
    }
    let n = |l: Level| checks.iter().filter(|c| c.level == l).count();
    eprintln!(
        "\n{} ok, {} warnings, {} problems",
        n(Level::Ok),
        n(Level::Warn),
        n(Level::Fail)
    );
}

pub async fn run(args: DoctorArgs) -> Result<bool, String> {
    let cfg = Arc::new(Config::load());
    let c = checks(cfg, args.online).await;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"checks": c})).map_err(|e| e.to_string())?
        );
    } else {
        print(&c);
    }
    Ok(!c.iter().any(|c| c.level == Level::Fail))
}

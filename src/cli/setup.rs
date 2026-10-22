//! `kritic setup`: from a bare binary to a working review in one command.
//!
//! It detects what the machine has, picks the defaults a careful user would pick, and asks only
//! what it cannot decide (and nothing at all with `--yes`). Every step is idempotent: running it
//! again resumes downloads, skips what is already in place and repairs what is missing. It ends
//! with `kritic doctor`, so the last thing on screen is what works and what doesn't.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use clap::{Args, ValueEnum};

use super::{detect, doctor, fetch, register, ui};
use crate::config::Config;

const EXAMPLE_CONFIG: &str = include_str!("../../examples/kritic.toml");
const KEV_REPO: &str = "mys/kev-4b-GGUF";
const LAYA_REPO: &str = "monatis/ggmlc";

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// Discover an already-installed local Kev only; never fall back to cloud
    Auto,
    /// Local Kev: downloads laya and the 4.5 GB model; nothing leaves the machine
    Local,
    /// Configured cloud provider: no model download, needs API access, sends code excerpts
    Cloud,
    /// No model: deterministic tools only
    Off,
}

#[derive(Args, Debug)]
pub struct SetupArgs {
    /// Accept every default without asking
    #[arg(short, long)]
    pub yes: bool,
    /// Who answers Kev's questions
    #[arg(long, value_enum)]
    pub backend: Option<BackendChoice>,
    /// laya build (default: detected from the GPU)
    #[arg(long, value_parser = ["cuda-sm89", "cuda-sm86", "cuda-sm80", "vulkan", "metal"])]
    pub laya: Option<String>,
    /// Kev quantization; the shipped gates were calibrated on q8_0
    #[arg(long, default_value = "q8_0", value_parser = ["q8_0", "ud_q4_k_m", "f16"])]
    pub quant: String,
    /// Install the vision model even with --yes (about 6 GB)
    #[arg(long)]
    pub vision: bool,
    /// Skip the vision model
    #[arg(long, conflicts_with = "vision")]
    pub no_vision: bool,
    /// Skip security scanners
    #[arg(long)]
    pub no_scanners: bool,
    /// Skip the web and desktop test harnesses
    #[arg(long)]
    pub no_harness: bool,
    /// Don't connect kritic to Claude Code or Hermes
    #[arg(long)]
    pub no_register: bool,
    /// Make kritic Hermes' context engine without asking
    #[arg(long)]
    pub hermes_context: bool,
}

struct Plan {
    yes: bool,
    home: PathBuf,
}

pub async fn run(args: SetupArgs) -> Result<(), String> {
    let cfg0 = Config::load();
    let plan = Plan {
        yes: args.yes,
        home: cfg0.home.clone(),
    };
    std::fs::create_dir_all(&plan.home)
        .map_err(|e| format!("cannot create {}: {e}", plan.home.display()))?;
    eprintln!(
        "kritic {} setup: {}",
        env!("CARGO_PKG_VERSION"),
        plan.home.display()
    );
    if !args.yes && !ui::interactive() {
        ui::info("no terminal attached: using defaults (same as --yes)");
    }
    let total = 6;
    let http = fetch::client();
    let mut problems: Vec<String> = Vec::new();

    // 1. config
    ui::heading(1, total, "Configuration");
    let cfg_path = cfg0
        .config_file
        .clone()
        .unwrap_or_else(|| plan.home.join("kritic.toml"));
    let mut doc: toml_edit::DocumentMut = std::fs::read_to_string(&cfg_path)
        .unwrap_or_else(|_| EXAMPLE_CONFIG.to_string())
        .parse()
        .map_err(|e| format!("{}: {e}", cfg_path.display()))?;

    // 2. Kev
    ui::heading(2, total, "Kev, the critic model");
    let gpu = detect::gpu();
    ui::info(&format!("GPU: {}", gpu.describe()));
    let key_available = cfg0.cloud_key().is_some();
    let selected = args.backend.unwrap_or(match cfg0.backend.as_str() {
        "local" => BackendChoice::Local,
        "cloud" => BackendChoice::Cloud,
        "auto" => BackendChoice::Auto,
        _ => BackendChoice::Off,
    });
    let backend = match selected {
        BackendChoice::Auto
            if gpu.fits_local_kev() && cfg0.laya_exe.is_file() && cfg0.kev_gguf.is_file() =>
        {
            BackendChoice::Local
        }
        BackendChoice::Auto => BackendChoice::Off,
        b => b,
    };
    if let Some(table) = doc.get_mut("kev").and_then(toml_edit::Item::as_table_mut) {
        table.remove("backend");
    }

    match backend {
        BackendChoice::Local => match install_local_kev(
            &http,
            &cfg0,
            &gpu,
            args.laya.as_deref(),
            &args.quant,
            &mut doc,
        )
        .await
        {
            Ok(()) => set(&mut doc, "review", "backend", "local"),
            Err(e) => {
                ui::fail(&e);
                problems.push(format!("local Kev: {e}"));
            }
        },
        BackendChoice::Cloud => {
            if key_available {
                ui::ok("configured cloud API key found");
            } else if let Some(k) = ui::secret(
                "Cloud API key (input hidden; stored in the kritic folder, never in kritic.toml)",
            ) {
                cfg0.save_cloud_key(&k)?;
                ui::ok(&format!("key saved to {}", cfg0.cloud_key_file.display()));
            } else {
                ui::warn("no key yet: run `kritic key` or set cloud.key_env");
                problems.push("cloud reviewer has no key yet".into());
            }
            ui::info(
                "Code excerpts under review will be sent to your configured endpoint. Cloud judgments are advisory.",
            );
            set(&mut doc, "review", "backend", "cloud");
        }
        BackendChoice::Off => {
            ui::skip("no model: model-only tools are hidden; deterministic tools remain available");
            set(&mut doc, "review", "backend", "off");
        }
        BackendChoice::Auto => unreachable!("auto is resolved above"),
    }
    write_config(&cfg_path, &doc)?;
    ui::ok(&format!("config: {}", cfg_path.display()));

    // 3. vision
    ui::heading(3, total, "Vision model (visual_critic, ux_critic novice)");
    if args.no_vision || (!args.vision && cfg0.vision_backend != "local") {
        ui::skip("skipped (--no-vision)");
    } else {
        let mut vision_cfg = cfg0.clone();
        vision_cfg.vision_backend = "local".into();
        if let Err(e) = install_vision(&vision_cfg, &plan, args.vision).await {
            ui::warn(&e);
        } else {
            set(&mut doc, "vision", "backend", "local");
            write_config(&cfg_path, &doc)?;
        }
    }

    // 4. harnesses
    ui::heading(4, total, "Test harnesses (browser, desktop)");
    if args.no_harness {
        ui::skip("skipped (--no-harness)");
    } else {
        if (cfg0.capabilities.get("web") == Some(&true)
            || cfg0.capabilities.get("ux") == Some(&true))
            && let Err(e) = install_browser(&http, &cfg0).await
        {
            ui::warn(&e);
        }
        if cfg!(windows)
            && cfg0.capabilities.get("desktop") == Some(&true)
            && let Err(e) = install_desktop_harness(&cfg0, &plan)
        {
            ui::warn(&e);
        }
    }

    // 5. scanners
    ui::heading(5, total, "Security scanners");
    if args.no_scanners || cfg0.capabilities.get("security") != Some(&true) {
        ui::skip("skipped (--no-scanners or security capability disabled)");
    } else {
        install_scanners(&http, &cfg0, &plan).await;
    }

    // 6. agents
    ui::heading(6, total, "Connect to your agent");
    let exe = doctor::exe_path();
    if args.no_register {
        ui::skip("skipped (--no-register)");
    } else {
        let mut any = false;
        if register::claude_present() {
            any = true;
            let rule = ui::confirm(
                "Add the end-of-task rule to ~/.claude/CLAUDE.md (a marked block kritic uninstall removes)?",
                true,
                plan.yes,
            );
            if let Err(e) = register::register_claude(
                &plan.home,
                &exe,
                &register::ClaudeOptions {
                    rule,
                    concise: crate::config::Config::load().output_style == "concise",
                },
            ) {
                ui::fail(&format!("Claude Code: {e}"));
                problems.push(format!("Claude Code: {e}"));
            }
        }
        if register::hermes_present() {
            any = true;
            let engine = args.hermes_context
                || ui::confirm(
                    "Let kritic manage Hermes' context (archive instead of summarise, with recall)?",
                    true,
                    plan.yes && args.hermes_context,
                );
            if let Err(e) = register::register_hermes(
                &plan.home,
                &exe,
                &register::HermesOptions {
                    context_engine: engine,
                    concise: crate::config::Config::load().output_style == "concise",
                },
            ) {
                ui::fail(&format!("Hermes: {e}"));
                problems.push(format!("Hermes: {e}"));
            }
        }
        if !any {
            ui::skip(
                "neither Claude Code nor Hermes found; register later with `kritic register --claude` or `--hermes`",
            );
        }
    }

    // summary
    eprintln!("\nChecking the result (kritic doctor):");
    let checks = doctor::checks(Arc::new(Config::load()), false).await;
    doctor::print(&checks);
    let asked = ui::prompts_asked();
    eprintln!(
        "\nSetup asked {asked} question{}.",
        if asked == 1 { "" } else { "s" }
    );
    eprintln!(
        "Next: review doctor findings; register the intended binary if needed, then restart your agent session."
    );
    if !problems.is_empty() {
        return Err(format!(
            "setup finished with problems: {}",
            problems.join("; ")
        ));
    }
    Ok(())
}

fn set(doc: &mut toml_edit::DocumentMut, section: &str, key: &str, value: &str) {
    if !doc.contains_table(section) {
        doc[section] = toml_edit::table();
    }
    doc[section][key] = toml_edit::value(value);
}

fn write_config(path: &Path, doc: &toml_edit::DocumentMut) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, doc.to_string())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn write_key(path: &Path, key: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, format!("{key}\n"))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// laya + (Windows CUDA) NVIDIA runtime DLLs + the Kev GGUF, each skipped when already present.
async fn install_local_kev(
    http: &reqwest::Client,
    cfg: &Config,
    gpu: &detect::Gpu,
    laya: Option<&str>,
    quant: &str,
    doc: &mut toml_edit::DocumentMut,
) -> Result<(), String> {
    if !cfg.config_errors.is_empty() {
        return Err("Fix config_errors before local model setup".into());
    }
    if cfg.kev_externally_managed {
        ui::skip(
            "Kev is externally managed; no service/model provisioning (probe with doctor --online)",
        );
        return Ok(());
    }
    if !gpu.fits_local_kev() {
        ui::warn(
            "this GPU is below the ~6 GB local Kev needs; expect slow answers (consider --backend cloud)",
        );
    }
    let backend = laya.unwrap_or_else(|| detect::laya_backend(gpu));
    // Space: the model plus laya and, on Windows CUDA, ~1 GB of runtime DLLs.
    let model = fetch::hf_file(http, KEV_REPO, &format!("kev_4b_{quant}.gguf")).await?;
    let need = if cfg.kev_gguf.is_file()
        && cfg
            .kev_gguf
            .file_name()
            .is_some_and(|n| n.to_string_lossy() == model.name)
    {
        0
    } else {
        model.size.unwrap_or(0)
    } + 1_500_000_000;
    if let Some(free) = detect::free_space(&cfg.home)
        && free < need
    {
        return Err(format!(
            "{} free on the disk holding {}, {} needed; set KRITIC_HOME to a bigger disk and run setup again",
            ui::bytes(free),
            cfg.home.display(),
            ui::bytes(need)
        ));
    }
    // laya
    let laya_ok = cfg.laya_exe.is_file();
    let laya_exe = if laya_ok {
        ui::ok(&format!("laya: {}", cfg.laya_exe.display()));
        cfg.laya_exe.clone()
    } else {
        let asset = detect::laya_asset(backend);
        ui::info(&format!("downloading laya ({asset})"));
        let remote = fetch::github_asset(http, LAYA_REPO, |n| n == asset).await?;
        let dir = cfg.home.join("laya");
        let archive = cfg.home.join("downloads").join(&remote.name);
        fetch::download(http, &remote, &archive).await?;
        if remote.name.ends_with(".zip") {
            fetch::extract_zip(&archive, &dir, |n| Some(PathBuf::from(n)))?;
        } else {
            fetch::extract_tar_gz(&archive, &dir)?;
        }
        let _ = std::fs::remove_file(&archive);
        let exe = find_file(&dir, if cfg!(windows) { "laya.exe" } else { "laya" }, 3)
            .ok_or("laya executable not found in the release archive")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
        }
        ui::ok(&format!("laya: {}", exe.display()));
        exe
    };
    set(doc, "kev", "laya_exe", &register::portable(&laya_exe));
    // CUDA runtime DLLs beside laya.exe (Windows CUDA builds only).
    if cfg!(windows) && backend.starts_with("cuda") {
        let dir = laya_exe.parent().unwrap_or(Path::new(".")).to_path_buf();
        for (pkg, dll) in [
            ("nvidia-cuda-runtime-cu12", "cudart64_12.dll"),
            ("nvidia-cublas-cu12", "cublas64_12.dll"),
        ] {
            if dir.join(dll).is_file() {
                ui::ok(&format!("{dll} present"));
                continue;
            }
            ui::info(&format!(
                "downloading {pkg} (NVIDIA's CUDA 12 runtime, from PyPI)"
            ));
            let wheel = fetch::pypi_wheel(http, pkg, "12.9", "win_amd64").await?;
            let path = cfg.home.join("downloads").join(&wheel.name);
            fetch::download(http, &wheel, &path).await?;
            let got = fetch::extract_zip(&path, &dir, |n| {
                (n.starts_with("nvidia/") && n.contains("/bin/") && n.ends_with(".dll"))
                    .then(|| PathBuf::from(n.rsplit('/').next().unwrap_or(n)))
            })?;
            let _ = std::fs::remove_file(&path);
            ui::ok(&format!("{} DLLs from {pkg}", got.len()));
        }
    }
    // Kev weights
    let dest = cfg.kev_gguf.parent().unwrap_or(&cfg.home).join(&model.name);
    ui::info(&format!(
        "Kev-4B weights: {} ({})",
        model.name,
        model.size.map(ui::bytes).unwrap_or_default()
    ));
    fetch::download(http, &model, &dest).await?;
    ui::ok(&format!("weights: {} (checksum verified)", dest.display()));
    set(doc, "kev", "gguf", &register::portable(&dest));
    Ok(())
}

fn find_file(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .find_map(|e| find_file(&e.path(), name, depth - 1))
}

fn run_visible(cmd: &mut Command, what: &str) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed ({status})"))
    }
}

async fn install_vision(cfg: &Config, plan: &Plan, forced: bool) -> Result<(), String> {
    if !cfg.config_errors.is_empty() {
        return Err("Fix config_errors before vision setup".into());
    }
    if cfg.vision_externally_managed {
        ui::skip(
            "Vision is externally managed; no service/model provisioning (probe with doctor --online)",
        );
        return Ok(());
    }
    let ollama = detect::tool("ollama", &[])
        .or_else(|| cfg.ollama_exe.is_file().then(|| cfg.ollama_exe.clone()));
    let Some(ollama) = ollama else {
        let installer = if cfg!(windows) && detect::tool("winget", &[]).is_some() {
            Some(vec![
                "winget",
                "install",
                "-e",
                "--id",
                "Ollama.Ollama",
                "--accept-source-agreements",
                "--accept-package-agreements",
            ])
        } else if cfg!(target_os = "macos") && detect::tool("brew", &[]).is_some() {
            Some(vec!["brew", "install", "ollama"])
        } else {
            None
        };
        let want = forced || (!plan.yes && ui::interactive());
        return match installer {
            Some(cmd) if want && ui::confirm(&format!("Ollama is not installed. Install it with `{}`?", cmd[..3].join(" ")), true, forced) => {
                run_visible(Command::new(cmd[0]).args(&cmd[1..]), "installing Ollama")?;
                Err("Ollama installed: run `kritic setup` again (a new terminal picks up its PATH) to pull the model".into())
            }
            _ => Err("Ollama not installed: visual_critic and the vision novice stay unavailable (https://ollama.com)".into()),
        };
    };
    let http = reqwest::Client::new();
    crate::vision::ensure_ollama(&http, cfg).await?;
    let has = http
        .get(format!("{}/api/tags", cfg.ollama_url))
        .send()
        .await
        .ok()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
        && Command::new(&ollama)
            .args(["show", &cfg.vision_model])
            .output()
            .is_ok_and(|o| o.status.success());
    if has {
        ui::ok(&format!("{} already pulled", cfg.vision_model));
        return Ok(());
    }
    // 6 GB: --yes alone doesn't pull it; --vision does, and an interactive user is asked.
    let pull = forced
        || (!plan.yes
            && ui::interactive()
            && ui::confirm(
                &format!("Pull {} (about 6 GB)?", cfg.vision_model),
                true,
                false,
            ));
    if !pull {
        return Err(format!(
            "not pulled (6 GB): run `kritic setup --vision` or `ollama pull {}`",
            cfg.vision_model
        ));
    }
    run_visible(
        Command::new(&ollama).args(["pull", &cfg.vision_model]),
        "ollama pull",
    )?;
    ui::ok(&format!("{} pulled", cfg.vision_model));
    Ok(())
}

/// A browser for the web tester: an installed Chrome, Chromium or Edge is used as is; otherwise
/// Chrome for Testing's headless shell (about 100 MB) goes into `<home>/browsers`.
async fn install_browser(http: &reqwest::Client, cfg: &Config) -> Result<(), String> {
    if let Ok(b) = crate::native::web::find_browser(cfg) {
        ui::ok(&format!("browser: {}", b.display()));
        return Ok(());
    }
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => "win64",
        ("linux", "x86_64") => "linux64",
        ("macos", "aarch64") => "mac-arm64",
        ("macos", _) => "mac-x64",
        (os, arch) => {
            return Err(format!(
                "no Chrome or Edge found and no Chrome for Testing build for {os}/{arch}: install Chrome or set ui.browser"
            ));
        }
    };
    let index = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";
    let v = fetch::get_json(http, index).await?;
    let stable = &v["channels"]["Stable"];
    let url = stable["downloads"]["chrome-headless-shell"]
        .as_array()
        .and_then(|a| a.iter().find(|d| d["platform"] == platform))
        .and_then(|d| d["url"].as_str())
        .ok_or_else(|| format!("Chrome for Testing has no headless shell for {platform}"))?
        .to_string();
    let version = stable["version"].as_str().unwrap_or("stable");
    let dir = cfg.home.join("browsers");
    let archive = cfg
        .home
        .join("downloads")
        .join(format!("chrome-headless-shell-{version}-{platform}.zip"));
    // Chrome for Testing publishes no checksums; the download is over HTTPS from Google's storage.
    fetch::download(
        http,
        &fetch::Remote {
            name: format!("Chrome for Testing {version}"),
            url,
            sha256: None,
            size: None,
        },
        &archive,
    )
    .await?;
    let _ = std::fs::remove_dir_all(&dir);
    fetch::extract_zip(&archive, &dir, |n| Some(PathBuf::from(n)))?;
    let _ = std::fs::remove_file(&archive);
    let exe = crate::native::web::find_browser(cfg)
        .map_err(|e| format!("Chrome for Testing unpacked but not found: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for f in walk_files(exe.parent().unwrap_or(&dir)) {
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.contains('.') || name.ends_with(".so") {
                let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
    ui::ok(&format!(
        "browser: Chrome for Testing {version} ({})",
        exe.display()
    ));
    Ok(())
}

#[cfg(unix)]
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_files(&p))
        } else {
            out.push(p)
        }
    }
    out
}

fn install_desktop_harness(cfg: &Config, plan: &Plan) -> Result<(), String> {
    let uv = match detect::tool(&cfg.uv_exe, &[]) {
        Some(uv) => uv,
        None => {
            if detect::tool("winget", &[]).is_some()
                && ui::confirm(
                    "uv (Python tool runner) is not installed. Install it with winget?",
                    true,
                    plan.yes,
                )
            {
                run_visible(
                    Command::new("winget").args([
                        "install",
                        "-e",
                        "--id",
                        "astral-sh.uv",
                        "--accept-source-agreements",
                        "--accept-package-agreements",
                    ]),
                    "installing uv",
                )?;
                return Err("uv installed: run `kritic setup` again in a new terminal to finish the desktop harness".into());
            }
            return Err("uv not found: desktop_test, resource_critic and hygiene_critic stay unavailable (https://docs.astral.sh/uv/)".into());
        }
    };
    ui::info("preparing the desktop harness's Python environment (uv)");
    run_visible(
        Command::new(uv)
            .args(["sync", "--quiet"])
            .current_dir(&cfg.harness_py),
        "uv sync",
    )?;
    ui::ok("desktop harness ready");
    Ok(())
}

async fn install_scanners(http: &reqwest::Client, cfg: &Config, plan: &Plan) {
    let bin = cfg.tools_bin.clone();
    let _ = std::fs::create_dir_all(&bin);
    let (os, arch) = if cfg!(windows) {
        ("windows", "x64")
    } else if cfg!(target_os = "macos") {
        (
            "darwin",
            if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "x64"
            },
        )
    } else {
        ("linux", "x64")
    };
    // gitleaks: a single binary in an archive.
    if crate::scan::tool_path(&bin, "gitleaks").is_some() {
        ui::ok("gitleaks present");
    } else {
        let r = async {
            let remote = fetch::github_asset(http, "gitleaks/gitleaks", |n| {
                n.contains(&format!("_{os}_{arch}"))
                    && (n.ends_with(".zip") || n.ends_with(".tar.gz"))
            })
            .await?;
            let archive = cfg.home.join("downloads").join(&remote.name);
            fetch::download(http, &remote, &archive).await?;
            let exe = if cfg!(windows) {
                "gitleaks.exe"
            } else {
                "gitleaks"
            };
            if remote.name.ends_with(".zip") {
                fetch::extract_zip(&archive, &bin, |n| (n == exe).then(|| PathBuf::from(n)))?;
            } else {
                let tmp = cfg.home.join("downloads").join("gitleaks-x");
                fetch::extract_tar_gz(&archive, &tmp)?;
                std::fs::copy(tmp.join(exe), bin.join(exe)).map_err(|e| e.to_string())?;
                let _ = std::fs::remove_dir_all(&tmp);
            }
            let _ = std::fs::remove_file(&archive);
            Ok::<_, String>(())
        }
        .await;
        match r {
            Ok(()) => ui::ok("gitleaks installed"),
            Err(e) => ui::warn(&format!("gitleaks: {e}")),
        }
    }
    // osv-scanner: a bare binary per platform.
    if crate::scan::tool_path(&bin, "osv-scanner").is_some() {
        ui::ok("osv-scanner present");
    } else {
        let arch2 = if arch == "x64" { "amd64" } else { arch };
        let want = format!(
            "osv-scanner_{os}_{arch2}{}",
            if cfg!(windows) { ".exe" } else { "" }
        );
        let dest = bin.join(if cfg!(windows) {
            "osv-scanner.exe"
        } else {
            "osv-scanner"
        });
        let r = async {
            let remote = fetch::github_asset(http, "google/osv-scanner", |n| n == want).await?;
            fetch::download(http, &remote, &dest).await?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
            }
            Ok::<_, String>(())
        }
        .await;
        match r {
            Ok(()) => ui::ok("osv-scanner installed"),
            Err(e) => ui::warn(&format!("osv-scanner: {e}")),
        }
    }
    // Python scanners through uv's tool installer.
    let missing: Vec<&str> = ["ruff", "bandit", "semgrep"]
        .into_iter()
        .filter(|t| crate::scan::tool_path(&bin, t).is_none())
        .collect();
    if missing.is_empty() {
        ui::ok("ruff, bandit, semgrep present");
        return;
    }
    let Some(uv) = detect::tool(&cfg.uv_exe, &[]) else {
        ui::warn(&format!(
            "{} need uv to install (https://docs.astral.sh/uv/)",
            missing.join(", ")
        ));
        return;
    };
    if !ui::confirm(
        &format!(
            "Install {} with `uv tool install` (semgrep is ~150 MB)?",
            missing.join(", ")
        ),
        true,
        plan.yes,
    ) {
        ui::skip("Python scanners skipped");
        return;
    }
    for t in missing {
        match Command::new(&uv)
            .args(["tool", "install", "--quiet", t])
            .status()
        {
            Ok(s) if s.success() => ui::ok(&format!("{t} installed")),
            _ => ui::warn(&format!("{t}: uv tool install failed")),
        }
    }
    ui::info(
        "uv puts tools in its bin folder; if a new terminal doesn't find them, run `uv tool update-shell`",
    );
}

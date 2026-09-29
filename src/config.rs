//! Runtime configuration.
//!
//! Every setting is resolved in this order: environment variable (`KRITIC_*`), then the config
//! file, then the built-in default. The config file is `$KRITIC_CONFIG` if set, else
//! `<home>/kritic.toml`, where `<home>` is `$KRITIC_HOME` or `~/.kritic`. See
//! `examples/kritic.toml` for every key with its default and meaning.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    /// Data directory: models, logs, run artifacts and optional tool binaries default to it.
    pub home: PathBuf,
    /// The config file that was read, if any.
    pub config_file: Option<PathBuf>,
    /// Problems reading the config file (bad TOML, unknown keys). Reported by `status`.
    pub config_errors: Vec<String>,
    /// Who answers Kev questions: `auto` (local Kev when installed, else cloud Jev when a key is
    /// configured), `local`, `cloud`, or `off` (deterministic tools only).
    pub backend: String,
    /// Agent reply style kritic installs: "concise" (default) or "default" (leave the agent alone).
    pub output_style: String,
    /// Cloud Jev (TypeSafe's model through an AI Gateway `/v1/evaluate` endpoint).
    pub cloud_url: String,
    pub cloud_model: String,
    /// File holding the gateway key when `AI_GATEWAY_API_KEY` is not set.
    pub cloud_key_file: PathBuf,
    pub laya_url: String,
    pub laya_exe: PathBuf,
    pub kev_gguf: PathBuf,
    pub laya_port: u16,
    pub model: String,
    /// Characters per code chunk. Kev's tokenizer sees ~1.6 chars/token on code and the
    /// sequence limit is 2,048 tokens, so ~1,300 chars leaves room for task + instructions.
    pub chunk_chars: usize,
    /// Max rows x tokens per laya request. Latency is flat below ~3k and falls off a cliff
    /// above ~3.7k on an RTX 5070 (measured: 2.8k -> 0.65s, 3.8k -> 4.8s, 4.7k -> 19s).
    pub token_budget: usize,
    pub max_seq_tokens: usize,
    pub max_revisions: u32,
    pub command_timeout_s: u64,
    pub log_path: PathBuf,
    pub cuda_cache: PathBuf,
    // ---- vision + UI testing
    pub ollama_url: String,
    pub ollama_exe: PathBuf,
    pub vision_model: String,
    /// Minimum per-criterion visual score (0-10) for approval. Calibrated: polished UI 7-9,
    /// plain unstyled 5-6, messy 0.
    pub visual_min: f64,
    /// Chrome or Edge for the native web tester. `None` finds an installed one.
    pub browser: Option<PathBuf>,
    pub runs_dir: PathBuf,
    pub ui_timeout_s: u64,
    // ---- analyzers, desktop + resource harness
    pub tools_bin: PathBuf,
    pub cargo_tools_bin: PathBuf,
    pub scan_timeout_s: u64,
    pub semgrep_config: String,
    pub max_triage: usize,
    pub max_hotspots: usize,
    pub uv_exe: String,
    pub harness_py: PathBuf,
    pub ws_port: u16,
    pub gpu_wait_s: u64,
    /// One Kev request, start to finish. The slowest of 6,399 logged requests took 5.3 s.
    pub kev_request_s: u64,
    /// A whole critic pass (every chunk plus completeness): past this it returns what it has.
    pub critic_deadline_s: u64,
    pub unity_editor: Option<PathBuf>,
    pub unity_hub_editors: PathBuf,
    pub context_limit: u64,
    /// MCP tools not offered to the client. Default: `map_eval`, the Beat Saber domain pack.
    pub disabled_tools: Vec<String>,
}

/// Every key the config file accepts, as `section.key`, with the environment variable that
/// overrides it. `examples/kritic.toml` documents each one; `tests/unit.rs` keeps them in sync.
pub const KEYS: &[(&str, &str)] = &[
    ("kev.backend", "KRITIC_KEV_BACKEND"),
    ("cloud.url", "KRITIC_CLOUD_URL"),
    ("cloud.model", "KRITIC_CLOUD_MODEL"),
    ("cloud.key_file", "KRITIC_CLOUD_KEY_FILE"),
    ("kev.laya_url", "KRITIC_LAYA_URL"),
    ("kev.laya_port", "KRITIC_LAYA_PORT"),
    ("kev.laya_exe", "KRITIC_LAYA_EXE"),
    ("kev.gguf", "KRITIC_KEV_GGUF"),
    ("kev.model", "KRITIC_KEV_MODEL"),
    ("kev.cuda_cache", "KRITIC_CUDA_CACHE"),
    ("kev.chunk_chars", "KRITIC_CHUNK_CHARS"),
    ("kev.token_budget", "KRITIC_TOKEN_BUDGET"),
    ("kev.max_seq_tokens", "KRITIC_MAX_SEQ_TOKENS"),
    ("kev.request_s", "KRITIC_KEV_REQUEST_S"),
    ("kev.critic_deadline_s", "KRITIC_CRITIC_DEADLINE_S"),
    ("vision.ollama_url", "KRITIC_OLLAMA_URL"),
    ("vision.ollama_exe", "KRITIC_OLLAMA_EXE"),
    ("vision.model", "KRITIC_VISION_MODEL"),
    ("vision.visual_min", "KRITIC_VISUAL_MIN"),
    ("review.max_revisions", "KRITIC_MAX_REVISIONS"),
    ("review.command_timeout_s", "KRITIC_COMMAND_TIMEOUT"),
    ("review.context_limit", "KRITIC_CONTEXT_LIMIT"),
    ("ui.browser", "KRITIC_BROWSER"),
    ("output.style", "KRITIC_OUTPUT_STYLE"),
    ("ui.timeout_s", "KRITIC_UI_TIMEOUT"),
    ("scan.tools_bin", "KRITIC_TOOLS_BIN"),
    ("scan.cargo_tools_bin", "KRITIC_CARGO_TOOLS_BIN"),
    ("scan.timeout_s", "KRITIC_SCAN_TIMEOUT"),
    ("scan.semgrep_config", "KRITIC_SEMGREP_CONFIG"),
    ("scan.max_triage", "KRITIC_MAX_TRIAGE"),
    ("scan.max_hotspots", "KRITIC_MAX_HOTSPOTS"),
    ("desktop.uv", "KRITIC_UV"),
    ("desktop.harness_py", "KRITIC_HARNESS_PY"),
    ("unity.editor", "KRITIC_UNITY_EDITOR"),
    ("unity.hub_editors", "KRITIC_UNITY_HUB_EDITORS"),
    ("server.ws_port", "KRITIC_WS_PORT"),
    ("server.gpu_wait_s", "KRITIC_GPU_WAIT"),
    ("server.log", "KRITIC_LOG"),
    ("server.runs_dir", "KRITIC_RUNS_DIR"),
    ("tools.disabled", "KRITIC_DISABLED_TOOLS"),
];

/// Tools off unless the config turns them on: domain packs that most projects don't need.
pub const DEFAULT_DISABLED: &[&str] = &["map_eval"];

/// An environment variable, treating set-but-empty (`KRITIC_HOME=`) as unset.
pub fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn user_home() -> PathBuf {
    env_var("USERPROFILE")
        .or_else(|| env_var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// `$KRITIC_HOME`, else `~/.kritic`.
pub fn kritic_home() -> PathBuf {
    let home = env_var("KRITIC_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home().join(".kritic"));
    // A relative KRITIC_HOME would scatter files wherever kritic happens to be started.
    if home.is_absolute() {
        home
    } else {
        std::env::current_dir()
            .map(|d| d.join(&home))
            .unwrap_or(home)
    }
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// First `name` on PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    let exe = exe_name(name);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
}

/// The harness directory: `harness/` beside the binary (release archives), else the source tree
/// the binary was built from.
fn default_harness_dir() -> PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("harness")))
        && dir.join("py").join("ux_adapter.py").is_file()
    {
        return dir;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("harness")
}

/// `<home>/laya/laya[.exe]`, or one directory below it (archives unpacked into a folder).
fn default_laya(home: &Path) -> PathBuf {
    let dir = home.join("laya");
    let direct = dir.join(exe_name("laya"));
    if direct.is_file() {
        return direct;
    }
    std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join(exe_name("laya")))
        .find(|p| p.is_file())
        .unwrap_or(direct)
}

fn default_ollama_exe() -> PathBuf {
    if let Some(p) = which("ollama") {
        return p;
    }
    if cfg!(windows) {
        let local = env_var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        local.join("Programs").join("Ollama").join("ollama.exe")
    } else {
        PathBuf::from("/usr/local/bin/ollama")
    }
}

fn default_unity_hub() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\Unity\Hub\Editor")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Applications/Unity/Hub/Editor")
    } else {
        user_home().join("Unity").join("Hub").join("Editor")
    }
}

/// Resolves one key: env, then file, then nothing.
struct Source {
    file: toml::Table,
}

impl Source {
    fn get(&self, key: &str) -> Option<String> {
        let env = KEYS
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, e)| *e)
            .expect("key listed in KEYS");
        if let Some(v) = env_var(env) {
            return Some(v);
        }
        let (section, name) = key.split_once('.').expect("section.key");
        match self.file.get(section)?.get(name)? {
            toml::Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }
    fn str(&self, key: &str, default: impl Into<String>) -> String {
        self.get(key).unwrap_or_else(|| default.into())
    }
    fn path(&self, key: &str, default: impl Into<PathBuf>) -> PathBuf {
        self.get(key)
            .map(PathBuf::from)
            .unwrap_or_else(|| default.into())
    }
    /// A list: a TOML array in the file, comma-separated in the environment.
    fn list(&self, key: &str, default: &[&str]) -> Vec<String> {
        let env = KEYS
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, e)| *e)
            .expect("key listed in KEYS");
        if let Some(v) = env_var(env) {
            return v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
        }
        let (section, name) = key.split_once('.').expect("section.key");
        match self.file.get(section).and_then(|t| t.get(name)) {
            Some(toml::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => default.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn num<T: std::str::FromStr>(&self, key: &str, default: T) -> T {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}

/// Reads the config file, collecting every problem instead of stopping at the first.
fn read_file(path: &Path) -> (toml::Table, Vec<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return (toml::Table::new(), vec![format!("{}: {e}", path.display())]),
    };
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => return (toml::Table::new(), vec![format!("{}: {e}", path.display())]),
    };
    let mut errors = Vec::new();
    for (section, value) in &table {
        let Some(inner) = value.as_table() else {
            errors.push(format!(
                "{}: `{section}` must be a [section]",
                path.display()
            ));
            continue;
        };
        for name in inner.keys() {
            let key = format!("{section}.{name}");
            if !KEYS.iter().any(|(k, _)| *k == key) {
                errors.push(format!("{}: unknown key `{key}`", path.display()));
            }
        }
    }
    (table, errors)
}

impl Config {
    /// Environment, then `kritic.toml`, then defaults.
    pub fn load() -> Self {
        let home = kritic_home();
        let explicit = env_var("KRITIC_CONFIG").map(PathBuf::from);
        let candidate = explicit.clone().unwrap_or_else(|| home.join("kritic.toml"));
        let (file, config_errors, config_file) = if candidate.is_file() || explicit.is_some() {
            let (t, e) = read_file(&candidate);
            (t, e, Some(candidate))
        } else {
            (toml::Table::new(), Vec::new(), None)
        };
        let s = Source { file };
        let port: u16 = s.num("kev.laya_port", 8089);
        let harness_dir = default_harness_dir();
        let mut config_errors = config_errors;
        let backend = s.str("kev.backend", "auto");
        if !["auto", "local", "cloud", "off"].contains(&backend.as_str()) {
            config_errors.push(format!(
                "kev.backend = \"{backend}\": expected auto, local, cloud or off (using auto)"
            ));
        }
        let output_style = s.str("output.style", "concise");
        if !["concise", "default"].contains(&output_style.as_str()) {
            config_errors.push(format!(
                "output.style = \"{output_style}\": expected concise or default (using concise)"
            ));
        }
        Config {
            output_style: if output_style == "default" {
                output_style
            } else {
                "concise".into()
            },
            backend: if ["auto", "local", "cloud", "off"].contains(&backend.as_str()) {
                backend
            } else {
                "auto".into()
            },
            cloud_url: s.str("cloud.url", "https://ai-gateway.vercel.sh/v1/evaluate"),
            cloud_model: s.str("cloud.model", "typesafe-ai/jev"),
            cloud_key_file: s.path("cloud.key_file", home.join("ai-gateway.key")),
            laya_url: s.str("kev.laya_url", format!("http://127.0.0.1:{port}")),
            laya_exe: s.path(
                "kev.laya_exe",
                which("laya").unwrap_or_else(|| default_laya(&home)),
            ),
            kev_gguf: s.path("kev.gguf", home.join("models").join("kev_4b_q8_0.gguf")),
            laya_port: port,
            model: s.str("kev.model", "kev-4b"),
            chunk_chars: s.num("kev.chunk_chars", 1300),
            token_budget: s.num("kev.token_budget", 2900),
            max_seq_tokens: s.num("kev.max_seq_tokens", 2048),
            max_revisions: s.num("review.max_revisions", 2),
            command_timeout_s: s.num("review.command_timeout_s", 300),
            log_path: s.path("server.log", home.join("logs").join("calls.jsonl")),
            cuda_cache: s.path("kev.cuda_cache", home.join("cuda-cache")),
            ollama_url: s.str("vision.ollama_url", "http://127.0.0.1:11434"),
            ollama_exe: s.path("vision.ollama_exe", default_ollama_exe()),
            vision_model: s.str("vision.model", "qwen2.5vl:7b"),
            visual_min: s.num("vision.visual_min", 6.0),
            browser: s.get("ui.browser").map(PathBuf::from),
            runs_dir: s.path("server.runs_dir", home.join("runs")),
            ui_timeout_s: s.num("ui.timeout_s", 300),
            tools_bin: s.path("scan.tools_bin", home.join("bin")),
            cargo_tools_bin: s.path(
                "scan.cargo_tools_bin",
                env_var("CARGO_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| user_home().join(".cargo"))
                    .join("bin"),
            ),
            scan_timeout_s: s.num("scan.timeout_s", 600),
            semgrep_config: s.str("scan.semgrep_config", "p/default"),
            max_triage: s.num("scan.max_triage", 25),
            max_hotspots: s.num("scan.max_hotspots", 10),
            uv_exe: s.str("desktop.uv", "uv"),
            harness_py: s.path("desktop.harness_py", harness_dir.join("py")),
            ws_port: s.num("server.ws_port", 8790),
            gpu_wait_s: s.num("server.gpu_wait_s", 900),
            kev_request_s: s.num("kev.request_s", 60),
            // 60 chunks at the measured ~2 s each is ~2 min; ten minutes means something stalled.
            critic_deadline_s: s.num("kev.critic_deadline_s", 600),
            unity_editor: s.get("unity.editor").map(PathBuf::from),
            unity_hub_editors: s.path("unity.hub_editors", default_unity_hub()),
            context_limit: s.num("review.context_limit", 1_000_000),
            disabled_tools: s.list("tools.disabled", DEFAULT_DISABLED),
            home,
            config_file,
            config_errors,
        }
    }

    /// The AI Gateway key for cloud Jev: `AI_GATEWAY_API_KEY`, else `cloud.key_file`. Never
    /// stored in `kritic.toml`, never logged.
    pub fn cloud_key(&self) -> Option<String> {
        if let Ok(k) = std::env::var("AI_GATEWAY_API_KEY")
            && !k.trim().is_empty()
        {
            return Some(k.trim().to_string());
        }
        std::fs::read_to_string(&self.cloud_key_file)
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
    }

    /// Alias of [`Config::load`]: environment variables still override the file.
    pub fn from_env() -> Self {
        Self::load()
    }
}

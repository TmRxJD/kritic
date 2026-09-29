//! MCP surface. Thin wrappers: parse params, call tool logic, audit, return typed JSON.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use schemars::JsonSchema;
use rmcp::{Json, ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;

use crate::config::Config;
use crate::critic::{CriticResult, critic, critic_diff};
use crate::kev::KevClient;
use crate::map::{MapResult, evaluate};
use crate::ui::{UiOptions, UiResult};
use crate::vision::VisualResult;
use crate::phase3::{Budgets, DesktopResult, QualityOptions, QualityResult, ResourceResult, SecurityResult};
use crate::tools::{
    RouteResult, StatusResult, TrimResult, ValidateInput, ValidateResult, artifact_validate, audit, context_trim,
    git_diff, load_files, route, status,
};

#[derive(Clone)]
pub struct Kritic {
    kev: Arc<KevClient>,
    /// One model on the GPU at a time: every model-backed tool (and the startup warm-up) holds
    /// this for its whole run, so Kev (laya) and the vision model never overlap.
    gpu: Arc<tokio::sync::Mutex<()>>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CriticParams {
    /// Score only the uncommitted changes (`git diff HEAD` + untracked files) under base_dir,
    /// limited to `paths` when given. Recommended in git repos: Kev judges changes better than
    /// whole files.
    #[serde(default)]
    pub git_diff: bool,
    /// Unified diff text to score (alternative to git_diff).
    #[serde(default)]
    pub diff: Option<String>,
    /// Absolute paths of every file created or changed by the task. Scored as whole files
    /// unless git_diff is true.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Raw artifact text, used only when `paths` is empty.
    #[serde(default)]
    pub artifact: Option<String>,
    /// The user's request in 1-3 sentences.
    #[serde(default)]
    pub task: String,
    /// Base directory for relative paths.
    #[serde(default)]
    pub base_dir: Option<String>,
    /// 0 for the first check, then 1, 2. revise_allowed is false at the cap.
    #[serde(default)]
    pub revision: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateParams {
    /// Absolute paths of every changed file (preferred).
    #[serde(default)]
    pub paths: Vec<String>,
    /// Raw content, used only when `paths` is empty.
    #[serde(default)]
    pub content: Option<String>,
    /// Kind for `content`: json | yaml | toml | python | code | map | text.
    #[serde(default)]
    pub kind: Option<String>,
    /// Project validators to run in base_dir, verbatim (e.g. "npx tsc --noEmit", "npm test -- --run").
    /// Output is returned verbatim; any non-zero exit forces approval=false.
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub base_dir: Option<String>,
    #[serde(default)]
    pub task: String,
    /// Run the Kev critic too. Set false if critic_score was already called on the same files.
    #[serde(default)]
    pub use_model: bool,
    #[serde(default)]
    pub revision: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MapParams {
    /// Path to a map file (Beat Saber difficulty .dat / JSON).
    #[serde(default)]
    pub path: Option<String>,
    /// Map JSON, used when `path` is empty.
    #[serde(default)]
    pub content: Option<String>,
    /// Song tempo; read from the Info.dat beside `path` when omitted.
    #[serde(default)]
    pub bpm: Option<f64>,
    /// Also ask Kev whether each 16-beat slice is awkward (spec map_slice; advisory, AUC 0.685
    /// measured 2026-09-27; about 0.2 s per slice). The deterministic checks decide approval.
    #[serde(default)]
    pub kev_slices: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RouteParams {
    pub task: String,
    #[serde(default)]
    pub hints: String,
    #[serde(default)]
    pub artifact_type: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TrimParams {
    pub query: String,
    /// Up to 64 chunks; each is judged on its first ~1,300 characters.
    pub chunks: Vec<String>,
    #[serde(default = "default_budget")]
    pub budget_chars: usize,
    #[serde(default = "default_min_rel")]
    pub min_relevance: f64,
    #[serde(default)]
    pub top_n: Option<usize>,
}
fn default_budget() -> usize {
    8000
}
fn default_min_rel() -> f64 {
    0.2
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VisualParams {
    /// Screenshot file to review (png/jpg).
    #[serde(default)]
    pub image_path: Option<String>,
    /// Or a URL to screenshot first (local hosts only unless allow_remote).
    #[serde(default)]
    pub url: Option<String>,
    /// Optional context: what the screen is for, target users, design system.
    #[serde(default)]
    pub metadata: String,
    #[serde(default = "yes")]
    pub full_page: bool,
    #[serde(default)]
    pub allow_remote: bool,
}
fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PersistCheck {
    /// CSS selector of an input/checkbox/select that should keep its value across reload.
    pub selector: String,
    /// Page to check (default: the entry URL).
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UiParams {
    /// App entrypoint, e.g. http://localhost:5173/ (local hosts only unless allow_remote).
    pub url: String,
    /// Extra routes to visit (relative or absolute), beyond same-origin links found on pages.
    #[serde(default)]
    pub routes: Vec<String>,
    /// Controls whose values must survive a reload.
    #[serde(default)]
    pub persist: Vec<PersistCheck>,
    #[serde(default = "five")]
    pub max_pages: usize,
    #[serde(default = "sixty")]
    pub max_actions: usize,
    /// Click controls that look destructive (delete, logout, pay...). Off by default.
    #[serde(default)]
    pub allow_destructive: bool,
    #[serde(default)]
    pub allow_remote: bool,
    /// Show the browser window.
    #[serde(default)]
    pub headed: bool,
    /// Run axe-core accessibility checks (default true).
    #[serde(default = "yes")]
    pub axe: bool,
    /// Treat serious/critical axe violations as breakages (default true).
    #[serde(default = "yes")]
    pub axe_blocking: bool,
    /// Control names (substring) to ignore for no_effect/error results, e.g. tooltips.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Scripted multi-step flows, each on a fresh page:
    /// [{name, url?, continue_on_error?, steps: [{do, target?, value?, timeout_ms?}]}].
    /// do: navigate | click | dblclick | fill | select | check | uncheck | press | hover | wait_for
    /// | wait | expect_text | expect_visible | expect_hidden | expect_url | screenshot.
    /// target: Playwright selector ("#id", "text=Save", "role=button[name='Save']").
    #[serde(default)]
    pub flows: Vec<serde_json::Value>,
    /// Crawl and exercise every control (default true). false = run only `flows`.
    #[serde(default = "yes")]
    pub explore: bool,
}
fn five() -> usize {
    5
}
fn sixty() -> usize {
    60
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SecurityParams {
    /// Project root to scan (git repo root recommended).
    pub base_dir: String,
    /// Ask Kev whether each code finding is a real, exploitable issue (ordering only; never
    /// removes a finding from the approval rule).
    #[serde(default = "yes")]
    pub triage: bool,
    /// Semgrep ruleset (default p/default; needs network the first time).
    #[serde(default)]
    pub semgrep_config: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QualityParams {
    pub base_dir: String,
    /// Files to measure (absolute or relative to base_dir). Empty = all source files (up to 80).
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default = "twenty")]
    pub max_complexity: usize,
    #[serde(default = "one_fifty")]
    pub max_function_lines: usize,
    /// Run the project's linters (ruff / clippy / eslint / tsc when configured).
    #[serde(default = "yes")]
    pub run_linters: bool,
    /// Kev maintainability judgment on the most complex functions.
    #[serde(default = "yes")]
    pub use_kev: bool,
}
fn twenty() -> usize {
    20
}
fn one_fifty() -> usize {
    150
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HygieneParams {
    /// Project root: processes whose executable or command line sits under it are the project's.
    pub base_dir: String,
    /// More directories the project's processes run from (e.g. a runtime or tools folder elsewhere).
    #[serde(default)]
    pub roots: Vec<String>,
    /// Process names that are expected to run and should not be listed (e.g. a dev server you keep up).
    #[serde(default)]
    pub ignore_names: Vec<String>,
    /// Globs for files a run should not leave behind (reported only, never deleted).
    #[serde(default)]
    pub leftover_globs: Vec<String>,
    /// Kill orphaned project processes and the children holding dead owners' sockets.
    #[serde(default)]
    pub clean: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResourceParams {
    /// Command to launch and measure (string for the shell, or argv array). Killed after sampling.
    #[serde(default)]
    pub command: Option<serde_json::Value>,
    /// Or measure an existing process (requires allow_attach=true).
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub allow_attach: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "thirty")]
    pub duration_s: f64,
    /// What the process is doing (Kev judges usage against it), e.g. "idle desktop app", "API under load test".
    #[serde(default)]
    pub workload: String,
    #[serde(default)]
    pub max_cpu_avg_pct: Option<f64>,
    #[serde(default)]
    pub max_rss_mb: Option<f64>,
    #[serde(default)]
    pub max_rss_slope_mb_per_min: Option<f64>,
    #[serde(default)]
    pub max_handles_growth: Option<i64>,
    #[serde(default)]
    pub max_gpu_util_avg_pct: Option<f64>,
}
fn thirty() -> f64 {
    30.0
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DesktopPersist {
    /// UI Automation AutomationId (WinForms: control Name; WPF: x:Name/AutomationProperties.AutomationId).
    #[serde(default)]
    pub auto_id: Option<String>,
    /// Or the control's accessible name.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DesktopParams {
    /// Executable of the app under test (a dev build from the project).
    pub exe: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Regex for the main window title, when the app opens several windows.
    #[serde(default)]
    pub window_title_re: Option<String>,
    /// Controls whose values must survive closing and relaunching the app.
    #[serde(default)]
    pub persist: Vec<DesktopPersist>,
    #[serde(default = "sixty")]
    pub max_actions: usize,
    #[serde(default)]
    pub allow_destructive: bool,
    /// Fall back to real mouse clicks for controls without UIA patterns (moves the cursor).
    #[serde(default)]
    pub use_mouse: bool,
    #[serde(default = "five_f")]
    pub hang_timeout_s: f64,
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Scripted multi-step flows, each in a fresh launch:
    /// [{name, continue_on_error?, steps: [{do, target?, value?, timeout_ms?}]}].
    /// do: click | fill | select | check | uncheck | toggle | press | wait_for | expect_text
    /// | expect_window | close_window | wait | screenshot.
    /// target: {name?, auto_id?, control_type?, window? (title regex)}; dialogs are searched too.
    #[serde(default)]
    pub flows: Vec<serde_json::Value>,
    /// Exercise every control (default true). false = run only `flows` (and persist checks).
    #[serde(default = "yes")]
    pub explore: bool,
    /// Unity player mode (auto when UnityPlayer.dll sits next to the exe). Unity draws its own UI,
    /// so instead of exercising controls: windowed launch with -logFile, `unity_steps` input,
    /// a `soak_s` watch for crashes/hangs, periodic screenshots, Player.log analysis, and
    /// per-frame timing via PresentMon (needs admin or Performance Log Users membership).
    #[serde(default)]
    pub unity: Option<bool>,
    #[serde(default = "twenty_f")]
    pub soak_s: f64,
    /// Posted input for Unity: [{do: "key", value: "space"|"escape"|"a"...} | {do: "click",
    /// target: {x: 0..1, y: 0..1}} | {do: "wait", value: ms}]. With `app_events`, also
    /// {do: "send", value: "<command line>", until?: regex, timeout_ms?} (sends over the app's
    /// socket and waits for the reply event; a reply with "outcome":"err" fails the step; the
    /// reference convention is "id|verb|argument") and {do: "await_event", value: regex, timeout_ms?}.
    #[serde(default)]
    pub unity_steps: Vec<serde_json::Value>,
    /// The app's own event WebSocket (e.g. "ws://127.0.0.1:4733"). Its events
    /// are relayed live to kritic's stream, drive `ready_event`, and carry `send` replies.
    #[serde(default)]
    pub app_events: Option<String>,
    /// Ready when an app event matches this regex (pushed, so no polling delay), e.g.
    /// "\"(detail|status)\":\"ready\"". Needs `app_events`.
    #[serde(default)]
    pub ready_event: Option<String>,
    /// Extra environment variables for the launched app (e.g. a harness directory).
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Let `app_events` point at a non-loopback host. Off: `send` steps push commands down this
    /// socket, so by default it must be the local app (127.0.0.1, localhost, ::1).
    #[serde(default)]
    pub allow_remote_app_events: bool,
    #[serde(default = "yes")]
    pub frame_timing: bool,
    #[serde(default)]
    pub min_avg_fps: Option<f64>,
    #[serde(default)]
    pub max_stutter_pct: Option<f64>,
    /// Unity: wait until this regex appears in the player log before soaking (e.g. the app's own
    /// "ready" marker), so the soak measures the running app, not its splash screen.
    #[serde(default)]
    pub ready_log: Option<String>,
    /// Unity: or wait until a window title matches this regex.
    #[serde(default)]
    pub ready_title: Option<String>,
    #[serde(default = "one_twenty")]
    pub ready_timeout_s: f64,
    /// Fail fast instead of waiting out `ready_timeout_s`: while starting, fail when the app goes
    /// this many seconds with no app event and no Player.log growth, or when `app_events` has not
    /// connected within this many seconds. 0 disables.
    #[serde(default = "fifteen_f")]
    pub stall_s: f64,
    /// Unity: fail the moment this regex appears in the player log (e.g. the app's own fatal marker).
    #[serde(default)]
    pub fail_log: Option<String>,
    #[serde(default = "five_f")]
    pub shot_every_s: f64,
}
fn fifteen_f() -> f64 {
    15.0
}
fn one_twenty() -> f64 {
    120.0
}
fn twenty_f() -> f64 {
    20.0
}
fn five_f() -> f64 {
    5.0
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UxParams {
    /// Repo root; the spec is read from <base_dir>/.kritic/ux.toml.
    pub base_dir: String,
    /// Or an explicit spec path.
    #[serde(default)]
    pub spec_path: Option<String>,
    /// web: override app.url (e.g. the dev server's port today).
    #[serde(default)]
    pub url: Option<String>,
    /// Exploration depth (default: largest task budget + 2, at most 6).
    #[serde(default)]
    pub max_depth: Option<usize>,
    /// Actions the exploration may take in total (each from a fresh reset plus replay).
    #[serde(default = "one_fifty")]
    pub max_actions: usize,
    /// auto (vision model, or Kev when Ollama is unavailable) | vision | kev | off.
    #[serde(default = "auto")]
    pub novice: String,
    /// Seeded novice runs per task (a primary task fails when the novice fails 2 of 3).
    #[serde(default = "three")]
    pub runs: usize,
    /// Any adapter call silent this long is a stall and ends the run (fail fast).
    #[serde(default = "fifteen_f")]
    pub stall_s: f64,
    /// Whole-run deadline.
    #[serde(default = "nine_hundred")]
    pub timeout_s: u64,
    #[serde(default)]
    pub allow_destructive: bool,
    #[serde(default)]
    pub allow_remote: bool,
    #[serde(default)]
    pub headed: bool,
}
fn auto() -> String {
    "auto".into()
}
fn three() -> usize {
    3
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PluginListParams {
    /// Repo root containing .kritic/plugins/.
    pub base_dir: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SpecLintParams {
    /// A decision spec as JSON (the same shape as specs/*.json and a plugin's `kev` block).
    #[serde(default)]
    pub spec: Option<serde_json::Value>,
    /// Or a path to a spec .json file.
    #[serde(default)]
    pub path: Option<String>,
    /// Also return the JSON Schema a spec must satisfy.
    #[serde(default)]
    pub with_schema: bool,
    /// List the built-in specs (name, purpose, questions and their gates) instead.
    #[serde(default)]
    pub builtin: bool,
}

#[derive(Debug, serde::Serialize, JsonSchema)]
pub struct SpecLintResult {
    pub tool: String,
    /// True when the spec parses and passes every lint rule.
    pub valid: bool,
    /// Every schema and lint problem at once, so a spec is fixed in one pass.
    pub errors: Vec<String>,
    /// Question ids that may fail approval (calibrated gates).
    pub gates: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builtin: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PluginRunParams {
    pub base_dir: String,
    /// Plugin name from plugin_list.
    pub name: String,
    /// Arguments for the plugin (see its input_schema).
    #[serde(default)]
    pub args: serde_json::Value,
    /// Hold the GPU lock while it runs (set when the plugin uses Kev/vision or other GPU models).
    #[serde(default)]
    pub gpu: bool,
}

#[derive(Debug, serde::Serialize, JsonSchema)]
pub struct PluginList {
    pub tool: String,
    pub plugin_dir: String,
    pub plugins: Vec<crate::plugins::Manifest>,
    /// Manifests that failed to load (fix these; they are not silently ignored).
    pub errors: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UnityTestParams {
    /// Unity project root (contains Assets/ and ProjectSettings/).
    pub project_path: String,
    /// EditMode (fast, no player) or PlayMode.
    #[serde(default = "edit_mode")]
    pub platform: String,
    /// Unity -testFilter (test name regex or full name).
    #[serde(default)]
    pub test_filter: Option<String>,
    #[serde(default = "nine_hundred")]
    pub timeout_s: u64,
}
fn edit_mode() -> String {
    "EditMode".into()
}
fn nine_hundred() -> u64 {
    900
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StatusParams {
    /// Start laya if it is down (can take ~60s after a reboot).
    #[serde(default)]
    pub start: bool,
}

/// Holds the GPU lock for one tool call and reports ownership on the event bus.
pub struct GpuGuard<'a> {
    _g: tokio::sync::MutexGuard<'a, ()>,
    _x: Option<crate::gpu::GpuFileLock>,
    tool: &'static str,
}
impl Drop for GpuGuard<'_> {
    fn drop(&mut self) {
        crate::events::ready("gpu", "free", self.tool);
    }
}

fn started(tool: &str) -> std::time::Instant {
    crate::events::emit("tool", serde_json::json!({"tool": tool, "phase": "start"}));
    std::time::Instant::now()
}

fn finished(tool: &str, t0: std::time::Instant, result: &impl serde::Serialize) {
    let v = serde_json::to_value(result).unwrap_or_default();
    crate::events::emit(
        "tool",
        serde_json::json!({"tool": tool, "phase": "end", "approval": v.get("approval"), "error": v.get("error"),
                           "duration_ms": t0.elapsed().as_millis() as u64}),
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TokenParams {
    /// Claude Code session id (transcript file stem). Default: the most recently active session.
    #[serde(default)]
    pub session_id: Option<String>,
    /// How many recent tasks (human turns) to include.
    #[serde(default = "ten")]
    pub last_tasks: usize,
    /// Context window of the session's model, in tokens (default KRITIC_CONTEXT_LIMIT, 1,000,000).
    #[serde(default)]
    pub context_limit: Option<u64>,
}
fn ten() -> usize {
    10
}

#[derive(Debug, serde::Serialize, JsonSchema)]
pub struct TokenMetrics {
    pub tool: String,
    pub session: crate::usage::SessionUsage,
    pub recent_tasks: Vec<crate::usage::TaskUsage>,
    /// Kev (laya) and vision (Ollama) usage since the session started.
    pub local_models: Vec<crate::usage::LocalModelStats>,
    /// How much context_trim was used this session and what it dropped.
    pub context_trim_calls: usize,
    /// Deterministic observations about context use.
    pub observations: Vec<String>,
}

#[tool_router(router = tool_router)]
impl Kritic {
    async fn gpu(&self, tool: &'static str) -> GpuGuard<'_> {
        let g = self.gpu.lock().await;
        // in-process first, then across sessions; if the cross-process wait times out the tool
        // still runs (slower if another session's model is resident) rather than failing
        let x = match crate::gpu::lock_across_processes(&self.kev.cfg, tool).await {
            Ok(l) => Some(l),
            Err(e) => {
                crate::events::ready("gpu", "contended", &e);
                None
            }
        };
        crate::events::ready("gpu", "held", tool);
        GpuGuard { _g: g, _x: x, tool }
    }

    pub fn new(cfg: Config) -> Self {
        let mut tool_router = Self::tool_router();
        for name in &cfg.disabled_tools {
            tool_router.disable_route(name.clone());
        }
        Kritic {
            kev: Arc::new(KevClient::new(Arc::new(cfg))),
            gpu: Arc::new(tokio::sync::Mutex::new(())),
            tool_router,
        }
    }

    /// Start laya in the background (under the GPU lock) so the first Kev call is fast.
    pub fn warm(&self) {
        let kev = self.kev.clone();
        let gpu = self.gpu.clone();
        tokio::spawn(async move {
            let _g = gpu.lock().await;
            let _x = crate::gpu::lock_across_processes(&kev.cfg, "warm-up").await.ok();
            if let Err(e) = kev.ensure_up().await {
                eprintln!("kritic: laya not started: {e}");
            }
        });
    }

    pub fn kev(&self) -> Arc<KevClient> {
        self.kev.clone()
    }

    /// Health check: laya/Kev reachable, paths, thresholds, log. Call first if another tool returns kev_ok=false.
    #[tool(name = "status")]
    pub async fn status_tool(&self, Parameters(p): Parameters<StatusParams>) -> Result<Json<StatusResult>, String> {
        Ok(Json(status(&self.kev, p.start).await))
    }

    /// End-of-task critic (Kev-4B). Prefer git_diff=true in a git repo: scores changed hunks
    /// ("does this change introduce a bug?"). Otherwise scores whole files in ~1,300-char chunks.
    /// Per unit: bug/correctness, severity, security, unfinished, quality, reviewer-injection;
    /// plus task completeness. Approval is computed from thresholds. Kev gives probabilities,
    /// not explanations: inspect `flagged_chunks` yourself. A coarse screen for obvious bugs,
    /// security patterns, missing work: NOT a substitute for tests. Advisory only.
    #[tool(name = "critic_score")]
    pub async fn critic_score(&self, Parameters(p): Parameters<CriticParams>) -> Result<Json<CriticResult>, String> {
        let t0 = started("critic_score");
        let cfg = &self.kev.cfg;
        let _gpu = self.gpu("critic_score").await;
        crate::gpu::unload_vision(cfg).await;
        if p.git_diff || p.diff.is_some() {
            let diff = match p.diff {
                Some(d) => d,
                None => git_diff(p.base_dir.as_deref(), &p.paths).await?,
            };
            let mut r = critic_diff(self.kev.as_ref(), cfg, &p.task, &diff, p.revision).await;
            r.flagged_chunks.truncate(15);
            r.flagged_chunks.truncate(15);
        finished("critic_score", t0, &r);
            audit(cfg, "critic_score", &r);
            return Ok(Json(r));
        }
        let mut skipped = Vec::new();
        let files = if !p.paths.is_empty() {
            let lf = load_files(&p.paths, p.base_dir.as_deref()).await;
            skipped = lf.skipped;
            lf.files
        } else {
            p.artifact.map(|a| vec![("<artifact>".to_string(), a)]).unwrap_or_default()
        };
        let mut r = critic(self.kev.as_ref(), cfg, &p.task, &files, p.revision).await;
        for s in skipped.iter().filter(|s| s.reason == "not found") {
            r.reasons.push(format!("missing file: {}", s.path));
            r.approval = Some(false);
        }
        finished("critic_score", t0, &r);
        audit(cfg, "critic_score", &r);
        Ok(Json(r))
    }

    /// Validate the finished file set: parse checks per file (JSON/YAML/TOML/Python syntax,
    /// merge markers, UTF-8), the project's own `commands` (tsc, lint, tests) run verbatim, then
    /// optionally the Kev critic. Any deterministic or command failure forces approval=false.
    #[tool(name = "artifact_validate")]
    pub async fn artifact_validate(&self, Parameters(p): Parameters<ValidateParams>) -> Result<Json<ValidateResult>, String> {
        let t0 = started("artifact_validate");
        let cfg = &self.kev.cfg;
        let _gpu = self.gpu("artifact_validate").await;
        crate::gpu::unload_vision(cfg).await;
        let r = artifact_validate(
            self.kev.as_ref(),
            cfg,
            ValidateInput {
                content: p.content.as_deref(),
                kind: p.kind.as_deref(),
                paths: &p.paths,
                base_dir: p.base_dir.as_deref(),
                commands: &p.commands,
                task: &p.task,
                use_model: p.use_model,
                revision: p.revision,
            },
        )
        .await;
        finished("artifact_validate", t0, &r);
        audit(cfg, "artifact_validate", &r);
        Ok(Json(r))
    }

    /// Evaluate a map. Beat Saber v2/v3 difficulties get deterministic stats (grid, time order,
    /// duplicates, parity breaks, density curve) plus Kev judgments on the stats and on 16-beat
    /// slices. Any heuristic issue forces approval=false.
    #[tool(name = "map_eval")]
    pub async fn map_eval(&self, Parameters(p): Parameters<MapParams>) -> Result<Json<MapResult>, String> {
        let t0 = started("map_eval");
        let cfg = &self.kev.cfg;
        let _gpu = self.gpu("map_eval").await;
        crate::gpu::unload_vision(cfg).await;
        let content = match (&p.path, &p.content) {
            (Some(path), _) => crate::files::read_lossy(std::path::Path::new(path)).map_err(|e| format!("cannot read {path}: {e}"))?,
            (None, Some(c)) => c.clone(),
            (None, None) => return Err("give path or content".into()),
        };
        let bpm = p.bpm.or_else(|| p.path.as_deref().and_then(|x| crate::map::bpm_beside(std::path::Path::new(x))));
        let r = evaluate(self.kev.as_ref(), cfg, &content, bpm, p.kev_slices).await;
        finished("map_eval", t0, &r);
        audit(cfg, "map_eval", &r);
        Ok(Json(r))
    }

    /// Second-opinion worker suggestion (local_worker | haiku | sonnet | opus) with calibrated
    /// probabilities. Never overrides the user's model-routing rules.
    #[tool(name = "route")]
    pub async fn route_tool(&self, Parameters(p): Parameters<RouteParams>) -> Result<Json<RouteResult>, String> {
        let t0 = started("route");
        let _gpu = self.gpu("route").await;
        crate::gpu::unload_vision(&self.kev.cfg).await;
        let r = route(self.kev.as_ref(), &p.task, &p.hints, &p.artifact_type).await;
        finished("route", t0, &r);
        audit(&self.kev.cfg, "route", &r);
        Ok(Json(r))
    }

    /// Rank context chunks by Kev relevance to `query`; keep the best within `budget_chars`,
    /// above `min_relevance`, optionally at most `top_n`. Extractive only (no summaries).
    #[tool(name = "context_trim")]
    pub async fn context_trim_tool(&self, Parameters(p): Parameters<TrimParams>) -> Result<Json<TrimResult>, String> {
        let t0 = started("context_trim");
        let cfg = &self.kev.cfg;
        let _gpu = self.gpu("context_trim").await;
        crate::gpu::unload_vision(cfg).await;
        let r = context_trim(self.kev.as_ref(), cfg, &p.query, &p.chunks, p.budget_chars, p.min_relevance, p.top_n).await;
        finished("context_trim", t0, &r);
        audit(cfg, "context_trim", &r);
        Ok(Json(r))
    }

    /// Visual critic (Qwen2.5-VL via Ollama): scores a screenshot 0-10 on layout clarity,
    /// spacing/alignment, visual hierarchy, color contrast, accessibility. Approval = every score
    /// >= 6. Issue phrases are `unverified_observations` (the model sometimes invents them); use
    /// ui_test's axe results for verified contrast/a11y problems. Pass image_path or url.
    /// Stops laya to free the GPU (Kev restarts on its next call, ~13s).
    #[tool(name = "visual_critic")]
    pub async fn visual_critic(&self, Parameters(p): Parameters<VisualParams>) -> Result<Json<VisualResult>, String> {
        let t0 = started("visual_critic");
        let cfg = &self.kev.cfg;
        let image = match (&p.image_path, &p.url) {
            (Some(path), _) => std::path::PathBuf::from(path),
            (None, Some(url)) => {
                let dir = crate::vision::run_dir(cfg, "visual");
                let out = dir.join("screenshot.png");
                crate::ui::screenshot(cfg, url, &out, p.full_page, p.allow_remote).await?;
                out
            }
            (None, None) => return Err("give image_path or url".into()),
        };
        let _gpu = self.gpu("visual_critic").await;
        let r = crate::vision::critique(cfg, &image, &p.metadata).await;
        finished("visual_critic", t0, &r);
        audit(cfg, "visual_critic", &r);
        Ok(Json(r))
    }

    /// Automated UI tester (Playwright, no LLM): opens a local web app, crawls same-origin pages,
    /// clicks/fills/toggles/selects every visible control, and reports crashes, page and console
    /// errors, failed requests, controls with no observable effect, persistence failures (for
    /// `persist` selectors), and axe-core accessibility violations, with screenshots. Skips
    /// destructive-looking controls unless allow_destructive. Any breakage => approval=false.
    #[tool(name = "ui_test")]
    pub async fn ui_test_tool(&self, Parameters(p): Parameters<UiParams>) -> Result<Json<UiResult>, String> {
        let t0 = started("ui_test");
        let cfg = &self.kev.cfg;
        let persist: Vec<serde_json::Value> =
            p.persist.iter().map(|c| serde_json::json!({"selector": c.selector, "url": c.url})).collect();
        let mut r = crate::ui::ui_test(
            cfg,
            &p.url,
            &p.routes,
            &persist,
            p.max_pages,
            p.max_actions,
            p.allow_destructive,
            p.allow_remote,
            p.headed,
            p.axe,
            &p.flows,
            p.explore,
            UiOptions { axe_blocking: p.axe_blocking, ignore: p.ignore },
        )
        .await;
        r.full_result = crate::tools::save_full(cfg, "ui_test", &r);
        r.report.interactions.retain(|i| !matches!(i.result.as_str(), "ok" | "navigated"));
        r.report.interactions.truncate(crate::tools::MAX_ITEMS);
        r.report.console_errors.truncate(crate::tools::MAX_ITEMS);
        r.report.failed_requests.truncate(crate::tools::MAX_ITEMS);
        finished("ui_test", t0, &r);
        audit(cfg, "ui_test", &r);
        Ok(Json(r))
    }

    /// Security critic: runs gitleaks (secrets), osv-scanner (vulnerable dependencies, any
    /// lockfile), semgrep (SAST), bandit (Python) and cargo-audit (Rust) where applicable; Kev
    /// triages each code finding with its source context. Approval=false on any high/critical
    /// finding (Kev triage never overrides that). Tools that aren't installed are listed as skipped.
    #[tool(name = "security_critic")]
    pub async fn security_critic(&self, Parameters(p): Parameters<SecurityParams>) -> Result<Json<SecurityResult>, String> {
        let t0 = started("security_critic");
        let cfg = &self.kev.cfg;
        let _gpu = self.gpu("security_critic").await;
        crate::gpu::unload_vision(cfg).await;
        let base = std::path::PathBuf::from(&p.base_dir);
        if !base.is_dir() {
            return Err(format!("base_dir is not a directory: {}", p.base_dir));
        }
        let sg = p.semgrep_config.unwrap_or_else(|| cfg.semgrep_config.clone());
        let mut r = crate::phase3::security(self.kev.as_ref(), cfg, &base, p.triage, &sg).await;
        r.full_result = crate::tools::save_full(cfg, "security_critic", &r);
        r.findings.truncate(crate::tools::MAX_ITEMS);
        finished("security_critic", t0, &r);
        audit(cfg, "security_critic", &r);
        Ok(Json(r))
    }

    /// Code-quality critic: the project's linters (ruff, clippy, eslint, tsc when configured) plus
    /// deterministic metrics (function length, complexity, nesting, duplicated blocks); Kev judges
    /// maintainability of the worst functions. Approval=false on linter errors or functions over
    /// max_complexity / max_function_lines.
    #[tool(name = "code_quality_critic")]
    pub async fn code_quality_critic(&self, Parameters(p): Parameters<QualityParams>) -> Result<Json<QualityResult>, String> {
        let t0 = started("code_quality_critic");
        let cfg = &self.kev.cfg;
        let _gpu = self.gpu("code_quality_critic").await;
        crate::gpu::unload_vision(cfg).await;
        let base = std::path::PathBuf::from(&p.base_dir);
        if !base.is_dir() {
            return Err(format!("base_dir is not a directory: {}", p.base_dir));
        }
        let paths = if p.paths.is_empty() { crate::tools::source_files(&base, crate::files::MAX_FILES) } else { p.paths.clone() };
        let lf = load_files(&paths, Some(&p.base_dir)).await;
        let files: Vec<(String, String)> = lf
            .files
            .into_iter()
            .map(|(f, t)| (std::path::Path::new(&f).strip_prefix(&base).map(|x| x.display().to_string()).unwrap_or(f), t))
            .collect();
        let mut r = crate::phase3::quality(
            self.kev.as_ref(),
            cfg,
            &base,
            &files,
            QualityOptions { max_complexity: p.max_complexity, max_function_lines: p.max_function_lines, run_linters: p.run_linters, use_kev: p.use_kev },
        )
        .await;
        r.full_result = crate::tools::save_full(cfg, "code_quality_critic", &r);
        r.lint_findings.truncate(crate::tools::MAX_ITEMS);
        finished("code_quality_critic", t0, &r);
        audit(cfg, "code_quality_critic", &r);
        Ok(Json(r))
    }

    /// Resource critic: launches `command` (or attaches to `pid` with allow_attach) and samples
    /// the process tree for duration_s: CPU, memory (RSS, private), handles, threads, disk I/O,
    /// plus whole-GPU utilization via nvidia-smi. Deterministic budget checks (defaults: memory
    /// growth > 5 MB/min, handles +500) and Kev's leak-risk / reasonableness judgment on the
    /// measurements. No admin/ETW; per-frame timing is not measured.
    #[tool(name = "resource_critic")]
    pub async fn resource_critic(&self, Parameters(p): Parameters<ResourceParams>) -> Result<Json<ResourceResult>, String> {
        let t0 = started("resource_critic");
        let cfg = &self.kev.cfg;
        if p.command.is_none() && p.pid.is_none() {
            return Err("give command or pid".into());
        }
        let budgets = Budgets {
            max_cpu_avg_pct: p.max_cpu_avg_pct,
            max_rss_mb: p.max_rss_mb,
            max_rss_slope_mb_per_min: p.max_rss_slope_mb_per_min,
            max_handles_growth: p.max_handles_growth,
            max_gpu_util_avg_pct: p.max_gpu_util_avg_pct,
        };
        // sample first (no GPU lock needed), then take the lock only for Kev's judgment
        let r = {
            let _gpu = self.gpu("resource_critic").await;
            crate::gpu::unload_vision(cfg).await;
            crate::phase3::resource(self.kev.as_ref(), cfg, p.command, p.pid, p.allow_attach, p.cwd.as_deref(), p.duration_s, &p.workload, budgets).await
        };
        let mut r = r;
        r.full_result = crate::tools::save_full(cfg, "resource_critic", &r);
        if let Some(arr) = r.timeline.as_array_mut() {
            let step = (arr.len() / 20).max(1);
            *arr = arr.iter().step_by(step).cloned().collect();
        }
        finished("resource_critic", t0, &r);
        audit(cfg, "resource_critic", &r);
        Ok(Json(r))
    }

    /// Hygiene critic (no model): what the project leaves running. Orphaned project processes
    /// (parent exited), listening sockets still bound after their owner exited (a child inherited
    /// the handle), and leftover files. `clean` kills orphans and socket inheritors; files are only
    /// reported. desktop_test and resource_critic run the same checks around every launch.
    #[tool(name = "hygiene_critic")]
    pub async fn hygiene_critic(&self, Parameters(p): Parameters<HygieneParams>) -> Result<Json<crate::hygiene::HygieneResult>, String> {
        let t0 = started("hygiene_critic");
        let cfg = &self.kev.cfg;
        let mut roots = vec![p.base_dir.clone()];
        roots.extend(p.roots);
        let mut r = crate::hygiene::audit(cfg, roots, &p.ignore_names, &p.leftover_globs, p.clean).await;
        r.full_result = crate::tools::save_full(cfg, "hygiene_critic", &r);
        r.reasons.truncate(crate::tools::MAX_ITEMS);
        finished("hygiene_critic", t0, &r);
        audit(cfg, "hygiene_critic", &r);
        Ok(Json(r))
    }

    /// Desktop app tester (Windows UI Automation via pywinauto, no model): launches the app,
    /// exercises every visible enabled control through UIA patterns (not the real mouse), and
    /// reports crashes (exit codes), hangs, error dialogs (with text), no-effect controls, stderr
    /// errors, and `persist` controls that lose their value across a relaunch. Skips
    /// destructive-looking controls and window chrome. Screenshots in out_dir.
    #[tool(name = "desktop_test")]
    pub async fn desktop_test_tool(&self, Parameters(p): Parameters<DesktopParams>) -> Result<Json<DesktopResult>, String> {
        if let Some(url) = &p.app_events
            && !p.allow_remote_app_events
        {
            crate::tools::require_loopback_ws(url)?;
        }
        let t0 = started("desktop_test");
        let cfg = &self.kev.cfg;
        let persist: Vec<serde_json::Value> = p.persist.iter().map(|c| serde_json::json!({"auto_id": c.auto_id, "name": c.name})).collect();
        let mut r = crate::phase3::desktop_test(
            cfg,
            &p.exe,
            &p.args,
            p.cwd.as_deref(),
            p.window_title_re.as_deref(),
            &persist,
            p.max_actions,
            p.allow_destructive,
            p.use_mouse,
            p.hang_timeout_s,
            &p.ignore,
            &p.flows,
            p.explore,
            &serde_json::json!({"unity": p.unity, "soak_s": p.soak_s, "unity_steps": p.unity_steps,
                                "frame_timing": p.frame_timing, "min_avg_fps": p.min_avg_fps, "max_stutter_pct": p.max_stutter_pct,
                                "ready_log": p.ready_log, "ready_title": p.ready_title, "ready_timeout_s": p.ready_timeout_s,
                                "stall_s": p.stall_s, "fail_log": p.fail_log,
                                "shot_every_s": p.shot_every_s, "app_events": p.app_events, "ready_event": p.ready_event,
                                "env": p.env}),
        )
        .await;
        r.full_result = crate::tools::save_full(cfg, "desktop_test", &r);
        if let Some(arr) = r.report.get_mut("interactions").and_then(|v| v.as_array_mut()) {
            arr.retain(|i| i["result"] != "ok");
            arr.truncate(crate::tools::MAX_ITEMS);
        }
        finished("desktop_test", t0, &r);
        audit(cfg, "desktop_test", &r);
        Ok(Json(r))
    }
    /// UX workflow critic: can a user find each mode and core action, and in how many steps?
    /// Reads the repo's `.kritic/ux.toml` (app, modes, tasks with deterministic done-checks, budgets)
    /// and drives the app (web: Playwright; desktop: UI Automation; unity: the app's event socket).
    /// Deterministic: a rule audit of every screen (labels, icon tooltips, 24 px targets, contrast,
    /// disabled-without-reason, a top-level entry per mode) and an exhaustive BFS giving each task's
    /// optimal_steps (unreachable or over budget fails). Novice: a weak vision model (Kev fallback)
    /// sees only the goal and a numbered screenshot; a primary task fails if it fails 2 of 3 runs.
    /// Hand `critic_brief` to the ux-critic agent. Treat hard fails as restructure findings.
    #[tool(name = "ux_critic")]
    pub async fn ux_critic(&self, Parameters(p): Parameters<UxParams>) -> Result<Json<crate::ux::UxResult>, String> {
        let t0 = started("ux_critic");
        let cfg = &self.kev.cfg;
        let path = p.spec_path.clone().map(std::path::PathBuf::from).unwrap_or_else(|| std::path::Path::new(&p.base_dir).join(".kritic").join("ux.toml"));
        let spec = crate::ux::load_spec(&path).map_err(|e| format!("ux spec invalid:
{}", e.join("
")))?;
        if !["auto", "vision", "kev", "off"].contains(&p.novice.as_str()) {
            return Err(format!("novice must be auto, vision, kev or off (got {})", p.novice));
        }
        if let Some(u) = spec.app.app_events.as_deref() {
            crate::tools::require_loopback_ws(u)?;
        }
        let out_dir = crate::vision::run_dir(cfg, "ux");
        let opts = crate::ux::Options {
            max_depth: p.max_depth.unwrap_or_else(|| crate::ux::default_depth(&spec)).max(1),
            max_actions: p.max_actions,
            runs: p.runs.max(1),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(p.timeout_s),
            out_dir: out_dir.clone(),
        };
        let mut run = crate::ux::Run::new(&spec, opts);
        let start = crate::ux::start_request(&spec, p.url.as_deref(), p.allow_destructive, p.allow_remote, p.headed, p.stall_s, &out_dir);
        let novice_requested = p.novice != "off";
        match crate::ux::ProcAdapter::launch(cfg, &spec, start, p.stall_s).await {
            Err(e) => run.fail(e),
            Ok(mut a) => {
                run.explore(&mut a).await;
                if novice_requested && run.wants_novice() {
                    // only the novice needs a model: hold the GPU for that part alone
                    let _gpu = self.gpu("ux_critic").await;
                    let vision_up = match p.novice.as_str() {
                        "kev" => Err("kev requested".to_string()),
                        _ => {
                            let http = reqwest::Client::new();
                            crate::vision::ensure_ollama(&http, cfg).await
                        }
                    };
                    match (p.novice.as_str(), vision_up) {
                        ("vision", Err(e)) => run.set_novice_error(&cfg.vision_model, e),
                        ("vision" | "auto", Ok(())) => {
                            crate::gpu::stop_laya(cfg).await;
                            run.novice(&mut a, &crate::ux::VisionNav { cfg }, &cfg.vision_model, true).await;
                        }
                        _ => {
                            crate::gpu::unload_vision(cfg).await;
                            run.novice(&mut a, &crate::ux::KevNav { d: self.kev.as_ref() }, "kev-4b (spec ux_novice_step)", false).await;
                        }
                    }
                }
                run.hygiene = a.close().await;
            }
        }
        let mut r = run.finish(novice_requested);
        // the full report sits beside its screenshots and carries the reviewer's brief too
        let full = out_dir.join("result.json");
        r.full_result = Some(full.display().to_string());
        r.critic_brief = crate::ux::critic_brief(&r, &spec);
        if std::fs::create_dir_all(&out_dir).and_then(|_| std::fs::write(&full, serde_json::to_vec_pretty(&r).unwrap_or_default())).is_err() {
            r.full_result = None;
        }
        r.rules.truncate(crate::tools::MAX_ITEMS);
        r.screens.truncate(crate::tools::MAX_ITEMS);
        finished("ux_critic", t0, &r);
        audit(cfg, "ux_critic", &r);
        Ok(Json(r))
    }

    /// Run a Unity project's Unity Test Framework tests headless (-batchmode -runTests) and parse
    /// the NUnit results. Uses the editor version from ProjectSettings/ProjectVersion.txt (Unity Hub
    /// install) or KRITIC_UNITY_EDITOR. The project must not be open in another editor.
    #[tool(name = "unity_tests")]
    pub async fn unity_tests(&self, Parameters(p): Parameters<UnityTestParams>) -> Result<Json<crate::unity::UnityTestResult>, String> {
        let t0 = started("unity_tests");
        let cfg = &self.kev.cfg;
        let r = crate::unity::run_tests(cfg, std::path::Path::new(&p.project_path), &p.platform, p.test_filter.as_deref(), p.timeout_s).await;
        finished("unity_tests", t0, &r);
        audit(cfg, "unity_tests", &r);
        Ok(Json(r))
    }

    /// Check a Kev decision spec before using it (built-in spec file or a plugin's `kev` block):
    /// schema, every field defined, every question naming the fields it reads in backticks with
    /// every answer defined, one condition per yes/no question, band questions using their
    /// vocabulary, and gates only with dated calibration evidence. Returns every error at once.
    /// with_schema returns the JSON Schema; builtin lists the built-in specs.
    #[tool(name = "spec_lint")]
    pub async fn spec_lint(&self, Parameters(p): Parameters<SpecLintParams>) -> Result<Json<SpecLintResult>, String> {
        let mut r = SpecLintResult { tool: "spec_lint".into(), valid: false, errors: vec![], gates: vec![], schema: None, builtin: None };
        if p.with_schema {
            r.schema = Some(crate::spec::json_schema());
        }
        if p.builtin {
            r.builtin = Some(serde_json::to_value(crate::spec::builtin()).unwrap_or_default());
            r.errors = crate::spec::lint_builtin();
            r.valid = r.errors.is_empty();
            return Ok(Json(r));
        }
        let text = match (&p.spec, &p.path) {
            (Some(v), _) => v.to_string(),
            (None, Some(path)) => std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?,
            (None, None) if p.with_schema => return Ok(Json(r)),
            (None, None) => return Err("give spec, path, builtin=true or with_schema=true".into()),
        };
        match crate::spec::load(&text) {
            Ok(spec) => {
                r.valid = true;
                r.gates = spec.questions.iter().filter(|q| spec.gates(&q.id)).map(|q| q.id.clone()).collect();
            }
            Err(errors) => r.errors = errors,
        }
        Ok(Json(r))
    }

    /// List repo-specific evaluator plugins in <base_dir>/.kritic/plugins/ (manifests: name,
    /// description, command, input_schema, optional Kev questions/rules). See plugin_run.
    #[tool(name = "plugin_list")]
    pub async fn plugin_list(&self, Parameters(p): Parameters<PluginListParams>) -> Result<Json<PluginList>, String> {
        let base = std::path::PathBuf::from(&p.base_dir);
        let (plugins, errors) = crate::plugins::discover(&base);
        Ok(Json(PluginList { tool: "plugin_list".into(), plugin_dir: crate::plugins::plugin_dir(&base).display().to_string(), plugins, errors }))
    }

    /// Run a repo plugin with the same guarantees as built-in tools: JSON contract validated,
    /// progress streamed to the event bus, optional Kev questions from the manifest's decision
    /// spec on the plugin's `kev_state`, approval computed from its approval, high/critical
    /// findings, and the spec's calibrated gates. Full output
    /// in full_result. Plugins are commands defined by the repo itself; run only in repos you trust.
    #[tool(name = "plugin_run")]
    pub async fn plugin_run(&self, Parameters(p): Parameters<PluginRunParams>) -> Result<Json<crate::plugins::PluginResult>, String> {
        let t0 = started("plugin_run");
        let cfg = &self.kev.cfg;
        let base = std::path::PathBuf::from(&p.base_dir);
        let uses_kev = crate::plugins::discover(&base).0.iter().any(|m| m.name == p.name && m.kev.is_some());
        let r = if p.gpu || uses_kev {
            let _gpu = self.gpu("plugin_run").await;
            crate::gpu::unload_vision(cfg).await;
            crate::plugins::run(self.kev.as_ref(), cfg, &base, &p.name, &p.args).await
        } else {
            crate::plugins::run(self.kev.as_ref(), cfg, &base, &p.name, &p.args).await
        };
        finished("plugin_run", t0, &r);
        audit(cfg, "plugin_run", &r);
        Ok(Json(r))
    }

    /// Token and context metrics. Claude side (from the Claude Code transcript): per-session and
    /// per-task input/output/cache tokens, context window occupancy and growth, compactions,
    /// cache hit rate, output tokens/sec. Local side: Kev and vision token counts and speeds.
    /// Plus how much context_trim was used.
    #[tool(name = "token_metrics")]
    pub async fn token_metrics(&self, Parameters(p): Parameters<TokenParams>) -> Result<Json<TokenMetrics>, String> {
        let cfg = &self.kev.cfg;
        let path = crate::usage::find_transcript(p.session_id.as_deref()).ok_or("no Claude Code transcript found")?;
        let limit = p.context_limit.unwrap_or(cfg.context_limit);
        let (session, tasks, calls) = crate::usage::analyze(&path, limit)?;
        let since = calls.first().and_then(|c| c.ts.parse::<chrono::DateTime<chrono::Utc>>().ok());
        let local_models = crate::usage::local_model_stats(cfg, since);
        let trim_calls = std::fs::read_to_string(&cfg.log_path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["tool"] == "context_trim")
            .filter(|v| since.is_none_or(|s| v["ts"].as_str().and_then(|t| t.parse::<chrono::DateTime<chrono::Utc>>().ok()).is_some_and(|t| t >= s)))
            .count();
        let mut obs = vec![format!(
            "context {} tokens = {:.1}% of {}; peak {}; growing ~{:.0} tokens/call over the last 20 calls",
            session.context_now, session.context_utilization_pct, session.context_limit, session.context_peak, session.context_growth_per_call
        )];
        obs.push(format!(
            "{} compactions; {} API calls processed {} input tokens in total ({}% served from cache)",
            session.compactions, session.api_calls, session.processed_input_tokens, session.cache_hit_rate_pct
        ));
        if trim_calls == 0 {
            obs.push("context_trim was not used this session: nothing was trimmed by kritic".into());
        }
        if session.context_utilization_pct > 50.0 {
            obs.push("context is over half the window: every call re-reads it (cached reads still count toward rate limits); consider /compact or a fresh session for the next task".into());
        }
        let n = tasks.len();
        let recent = tasks.into_iter().skip(n.saturating_sub(p.last_tasks)).collect();
        let r = TokenMetrics { tool: "token_metrics".into(), session, recent_tasks: recent, local_models, context_trim_calls: trim_calls, observations: obs };
        Ok(Json(r))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Kritic {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("kritic", env!("CARGO_PKG_VERSION")))
            .with_instructions(
            "kritic: advisory end-of-task review. After finishing a task that changed code/config/map files: \
             critic_score(git_diff or paths, task) then artifact_validate(paths, commands, use_model=false); map_eval for maps; \
             for web UIs also ui_test(url) and visual_critic(url); for any UI change ux_critic(base_dir) with the repo's \
             .kritic/ux.toml, then hand its critic_brief to the ux-critic agent (hard fails mean restructure, not patch). \
             Revise at most twice (revision=1,2). Never fabricate scores; if kev_ok=false report the critic as offline.",
        )
    }
}

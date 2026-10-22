//! ux_critic: judges a UI by workflow, not structure. Can a user find each mode and core action,
//! and how many steps does it take?
//!
//! Everything that decides approval is measured against deterministic done-checks the repo
//! declares in `.kritic/ux.toml`, never a model's opinion:
//! - a rule audit of every observed screen (labels, tooltips, target size, contrast, disabled
//!   controls that say why, overlapping or off-screen controls, a top-level entry for every mode);
//! - continuity: a screen has to be the same screen every time it is reached. Each replay of a
//!   path is compared with the first visit (controls, text, and the screenshots pixel for
//!   pixel); a difference is a finding with both pictures, and enough of them fail the run,
//!   because every measurement after a state leak is about a different app;
//! - an exhaustive breadth-first exploration (screen fingerprint -> action -> fingerprint) that
//!   gives each task its `optimal_steps`, the shortest path to a screen passing its done-check;
//! - a deliberately weak novice (vision model on a set-of-marks screenshot, or Kev through the
//!   `ux_novice_step` spec) that only sees the goal and the screen. It measures discoverability
//!   and fails a primary task only when it fails at least 2 of 3 seeded runs.
//!
//! Apps are driven through one adapter contract (observe / act / reset) served by a persistent
//! harness process: web (Playwright, harness/ux_web.mjs), desktop (UI Automation) and Unity
//! (the app's own event socket), both in harness/py/ux_adapter.py.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::Config;
use crate::kev::Decider;
use crate::ui::AxeViolation;

// ------------------------------------------------------------------ spec (.kritic/ux.toml)

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct AppSpec {
    /// web | desktop | unity
    pub kind: String,
    /// What the app is for, in a sentence or two (the reviewer judges against it).
    pub purpose: String,
    /// web: the entry URL (local hosts only unless allow_remote).
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub viewport: Option<[u32; 2]>,
    /// desktop / unity: the executable, its arguments, working directory and extra environment.
    #[serde(default)]
    pub exe: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub window_title_re: Option<String>,
    /// unity (required): the app's event WebSocket, which also carries harness commands.
    #[serde(default)]
    pub app_events: Option<String>,
    /// unity: ready when an app event matches this regex.
    #[serde(default)]
    pub ready_event: Option<String>,
    #[serde(default)]
    pub ready_timeout_s: Option<f64>,
    /// unity: where the app writes the screenshots its `shot` command names, when it answers with
    /// a bare file name instead of an absolute path.
    #[serde(default)]
    pub shot_dir: Option<String>,
    /// Wait after each load or action before observing (web default 150 ms).
    #[serde(default)]
    pub settle_ms: Option<u64>,
    /// unity / desktop: how long to wait for an app that reports it is busy (loading) before the
    /// wait counts as a failure. Default 45 s.
    #[serde(default)]
    pub busy_timeout_s: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct DoneSpec {
    /// A visible control whose label matches this regex.
    #[serde(default)]
    pub control: Option<String>,
    /// The screen's visible text matches this regex.
    #[serde(default)]
    pub text: Option<String>,
    /// web: the URL matches this regex.
    #[serde(default)]
    pub url: Option<String>,
    /// unity (or any app with app_events): an app event since the last reset matches this regex.
    #[serde(default)]
    pub event: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModeSpec {
    pub name: String,
    pub done: DoneSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub name: String,
    /// The goal in plain words, as a user would say it. The novice sees only this and the screen.
    pub goal: String,
    pub done: DoneSpec,
    #[serde(default)]
    pub budget: Option<usize>,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecFile {
    /// Regexes on control labels: never pressed by exploration or the novice (still audited).
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Regexes on text that changes while the screen stays the same (a status line, a counter, a
    /// loading message). Such text is left out of a screen's identity and of continuity
    /// comparisons; without it every tick of a clock is a new screen and the action budget goes on
    /// phantom duplicates.
    #[serde(default)]
    pub volatile: Vec<String>,
    /// Parts of the window that legitimately animate (a native video or preview surface), as
    /// [x0, y0, x1, y1] fractions of the window. They are masked out of screenshot comparisons.
    #[serde(default)]
    pub volatile_regions: Vec<[f64; 4]>,
    pub app: AppSpec,
    #[serde(default)]
    pub modes: Vec<ModeSpec>,
    #[serde(default)]
    pub tasks: Vec<TaskSpec>,
}

#[derive(Debug, Clone)]
pub struct Done {
    pub control: Option<Regex>,
    pub text: Option<Regex>,
    pub url: Option<Regex>,
    pub event: Option<Regex>,
}

impl Done {
    /// Every predicate given must hold on the observation.
    pub fn holds(&self, o: &Obs) -> bool {
        self.control
            .as_ref()
            .is_none_or(|r| o.controls.iter().any(|c| r.is_match(&c.label)))
            && self.text.as_ref().is_none_or(|r| r.is_match(&o.text))
            && self.url.as_ref().is_none_or(|r| r.is_match(&o.url))
            && self
                .event
                .as_ref()
                .is_none_or(|r| o.events.iter().any(|e| r.is_match(e)))
    }
}

/// A mode or a task: something the user must be able to reach.
#[derive(Debug, Clone)]
pub struct Goal {
    pub name: String,
    pub goal: String,
    pub done: Done,
    pub budget: usize,
    pub primary: bool,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub path: String,
    pub text: String,
    pub app: AppSpec,
    pub modes: Vec<Goal>,
    pub tasks: Vec<Goal>,
    pub ignore: Vec<String>,
    pub volatile: Vec<String>,
    pub volatile_regions: Vec<[f64; 4]>,
}

pub const DEFAULT_BUDGET: usize = 3;
/// Primary tasks take 3 steps or fewer (the ux-critic reviewer's bar); a spec cannot relax it.
pub const PRIMARY_MAX_BUDGET: usize = 3;
const KINDS: [&str; 3] = ["web", "desktop", "unity"];

fn rx(errors: &mut Vec<String>, at: &str, pattern: &Option<String>) -> Option<Regex> {
    let p = pattern.as_ref()?;
    match Regex::new(p) {
        Ok(r) => Some(r),
        Err(e) => {
            errors.push(format!(
                "{at}: invalid regex {p:?}: {}",
                e.to_string().lines().last().unwrap_or("")
            ));
            None
        }
    }
}

fn done(errors: &mut Vec<String>, at: &str, d: &DoneSpec, app: &AppSpec) -> Done {
    if d.control.is_none() && d.text.is_none() && d.url.is_none() && d.event.is_none() {
        errors.push(format!(
            "{at}.done: give at least one of control, text, url, event"
        ));
    }
    if d.url.is_some() && app.kind != "web" {
        errors.push(format!("{at}.done.url: only for kind = \"web\""));
    }
    if d.event.is_some() && app.app_events.is_none() {
        errors.push(format!("{at}.done.event: needs app.app_events"));
    }
    Done {
        control: rx(errors, &format!("{at}.done.control"), &d.control),
        text: rx(errors, &format!("{at}.done.text"), &d.text),
        url: rx(errors, &format!("{at}.done.url"), &d.url),
        event: rx(errors, &format!("{at}.done.event"), &d.event),
    }
}

/// Parse and validate a ux spec. Returns every problem at once.
pub fn parse_spec(text: &str, path: &str) -> Result<Spec, Vec<String>> {
    let f: SpecFile = toml::from_str(text).map_err(|e| vec![format!("{path}: {e}")])?;
    let mut errors = Vec::new();
    let app = &f.app;
    if !KINDS.contains(&app.kind.as_str()) {
        errors.push(format!(
            "app.kind: {:?} is not one of web, desktop, unity",
            app.kind
        ));
    }
    if app.purpose.trim().is_empty() {
        errors.push("app.purpose: say what the app is for".into());
    }
    match app.kind.as_str() {
        "web" if app.url.is_none() => errors.push("app.url: required for kind = \"web\"".into()),
        "desktop" | "unity" if app.exe.is_none() => {
            errors.push(format!("app.exe: required for kind = \"{}\"", app.kind))
        }
        _ => {}
    }
    if app.kind == "unity" && app.app_events.is_none() {
        errors.push(
            "app.app_events: required for kind = \"unity\" (the adapter drives the app over it)"
                .into(),
        );
    }
    rx(&mut errors, "app.window_title_re", &app.window_title_re);
    rx(&mut errors, "app.ready_event", &app.ready_event);
    for (i, p) in f.ignore.iter().enumerate() {
        rx(&mut errors, &format!("ignore[{i}]"), &Some(p.clone()));
    }
    for (i, p) in f.volatile.iter().enumerate() {
        rx(&mut errors, &format!("volatile[{i}]"), &Some(p.clone()));
    }
    if f.tasks.is_empty() {
        errors.push("tasks: declare at least one task".into());
    }
    let mut names = std::collections::HashSet::new();
    let mut modes = Vec::new();
    for (i, m) in f.modes.iter().enumerate() {
        let at = format!("modes[{i}] ({})", m.name);
        if m.name.trim().is_empty() || !names.insert(format!("mode:{}", m.name)) {
            errors.push(format!("{at}: name must be non-empty and unique"));
        }
        let d = done(&mut errors, &at, &m.done, app);
        modes.push(Goal {
            name: m.name.clone(),
            goal: format!("open {}", m.name),
            done: d,
            budget: 1,
            primary: true,
        });
    }
    let mut tasks = Vec::new();
    for (i, t) in f.tasks.iter().enumerate() {
        let at = format!("tasks[{i}] ({})", t.name);
        if t.name.trim().is_empty() || !names.insert(format!("task:{}", t.name)) {
            errors.push(format!("{at}: name must be non-empty and unique"));
        }
        if t.goal.trim().is_empty() {
            errors.push(format!(
                "{at}.goal: say what the user wants, in plain words"
            ));
        }
        let budget = t.budget.unwrap_or(DEFAULT_BUDGET);
        if budget == 0 {
            errors.push(format!("{at}.budget: must be at least 1"));
        }
        if t.primary && budget > PRIMARY_MAX_BUDGET {
            errors.push(format!("{at}.budget: a primary task takes at most {PRIMARY_MAX_BUDGET} steps (got {budget}); restructure the UI rather than raising it"));
        }
        let d = done(&mut errors, &at, &t.done, app);
        tasks.push(Goal {
            name: t.name.clone(),
            goal: t.goal.clone(),
            done: d,
            budget,
            primary: t.primary,
        });
    }
    if errors.is_empty() {
        Ok(Spec {
            path: path.into(),
            text: text.into(),
            app: f.app,
            modes,
            tasks,
            ignore: f.ignore,
            volatile: f.volatile,
            volatile_regions: f.volatile_regions,
        })
    } else {
        Err(errors)
    }
}

pub fn load_spec(path: &Path) -> Result<Spec, Vec<String>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| vec![format!("cannot read {}: {e}", path.display())])?;
    let mut spec = parse_spec(&text, &path.display().to_string())?;
    // a web url without a scheme is a file beside the spec (the calibration fixtures)
    if let Some(u) = spec.app.url.as_mut().filter(|u| !u.contains("://")) {
        let file = path.parent().unwrap_or(Path::new(".")).join(&*u);
        let abs = std::fs::canonicalize(&file)
            .map_err(|e| vec![format!("app.url: {}: {e}", file.display())])?;
        let s = abs.display().to_string();
        *u = format!(
            "file:///{}",
            s.trim_start_matches(r"\\?\").replace('\\', "/")
        );
    }
    Ok(spec)
}

// ------------------------------------------------------------------ adapter contract

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema, Default)]
pub struct Control {
    /// Stable across replays: role, label and index among controls sharing both.
    pub id: String,
    /// The accessible label (what assistive tech and the novice read); empty when it has none.
    #[serde(default)]
    pub label: String,
    /// Visible text; empty for an icon-only control.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub role: String,
    /// x, y, width, height in screen (viewport) pixels.
    #[serde(default)]
    pub rect: [f64; 4],
    #[serde(default = "yes")]
    pub enabled: bool,
    /// A link inside running text (exempt from the target-size rule).
    #[serde(default)]
    pub inline: bool,
    #[serde(default)]
    pub selected: bool,
    /// Why the adapter will not press it: destructive, ignored, external link...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<String>,
}
fn yes() -> bool {
    true
}

impl Control {
    pub fn actionable(&self) -> bool {
        self.enabled && self.skip.is_none()
    }
    pub fn shown(&self) -> String {
        if self.label.trim().is_empty() {
            "(no label)".into()
        } else {
            self.label.clone()
        }
    }
    fn centre(&self) -> (f64, f64) {
        (
            self.rect[0] + self.rect[2] / 2.0,
            self.rect[1] + self.rect[3] / 2.0,
        )
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Obs {
    pub fingerprint: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// Visible text of the screen (done `text` predicates read it).
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub viewport: [f64; 2],
    #[serde(default)]
    pub controls: Vec<Control>,
    #[serde(default)]
    pub screenshot: Option<String>,
    /// Set-of-marks screenshot: actionable controls numbered in order.
    #[serde(default)]
    pub marked: Option<String>,
    #[serde(default)]
    pub axe: Vec<AxeViolation>,
    /// App events since the last reset.
    #[serde(default)]
    pub events: Vec<String>,
    /// Each visible text on its own (`text` joins them), so two visits can be compared piece by piece.
    #[serde(default)]
    pub texts: Vec<String>,
    /// What the app says it is still waiting for (a loading veil, a preview starting); empty when it is
    /// ready. The adapter waits for it to clear, so a value here means the wait ran out.
    #[serde(default)]
    pub busy: String,
    /// How much this screenshot differs from the one it was asked to compare with.
    #[serde(default)]
    pub pixel_diff: Option<PixelDiff>,
}

/// Two screenshots compared: the fraction of pixels that changed and the box holding every change.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema, Default)]
pub struct PixelDiff {
    pub fraction: f64,
    /// x0, y0, x1, y1 in screenshot pixels.
    #[serde(default)]
    pub bbox: [f64; 4],
    #[serde(default)]
    pub sizes_differ: bool,
}

/// A screen that was not the same screen when it was reached again, or was seen mid-load.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Continuity {
    /// replay_differs | looks_different | seen_busy
    pub kind: String,
    pub screen: usize,
    pub path: Vec<String>,
    pub detail: String,
    /// What the second visit has that the first did not, and the reverse, and what changed in place.
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    /// The two pictures to open side by side: the first visit and the repeat.
    pub before: Option<String>,
    pub after: Option<String>,
    pub pixel_diff: Option<PixelDiff>,
}

/// A replay whose screenshot changes by more than this, with no structural difference, is
/// reported as looking different: something drew or failed to draw that no control describes.
const VISUAL_DIFF: f64 = 0.30;
/// This share of the screens a run expanded changing between visits fails the run.
const UNSTABLE_SHARE: f64 = 0.1;

impl Obs {
    pub fn actionable(&self) -> Vec<&Control> {
        self.controls.iter().filter(|c| c.actionable()).collect()
    }
}

/// What the engine needs from an app, whatever its kind.
pub trait Adapter: Send {
    /// Back to the start state (fresh page, relaunch, or the app's own home command).
    fn reset(&mut self) -> impl Future<Output = Result<(), String>> + Send;
    /// `opts`: {shot?: path, marks?: path, axe?: bool}.
    fn observe(&mut self, opts: Value) -> impl Future<Output = Result<Obs, String>> + Send;
    fn act(&mut self, id: &str) -> impl Future<Output = Result<(), String>> + Send;
}

/// Picks the next control for the novice: an index into `labels`. `pressed` is what it already
/// pressed this run, in order: a first-time user still remembers their own clicks.
pub trait Navigator: Sync {
    fn choose(
        &self,
        goal: &str,
        pressed: &[String],
        image: Option<&str>,
        labels: &[String],
        seed: u64,
    ) -> impl Future<Output = Result<usize, String>> + Send;
}

// ------------------------------------------------------------------ harness process adapter

/// A persistent harness process speaking JSON lines: one request per line on stdin, one reply per
/// line on stdout, progress events on stderr. Every call has a stall limit; a stall is an error,
/// never a wait. The process tree is killed if it does not close cleanly.
pub struct ProcAdapter {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    out: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    err: Option<tokio::task::JoinHandle<String>>,
    stall: Duration,
    slow: Duration,
}

impl ProcAdapter {
    /// Start the harness for `spec.app.kind` and send it `start`.
    pub async fn launch(
        cfg: &Config,
        spec: &Spec,
        start: Value,
        stall_s: f64,
    ) -> Result<Self, String> {
        let mut cmd = tokio::process::Command::new(&cfg.uv_exe);
        cmd.args(["run", "--quiet", "--project"])
            .arg(&cfg.harness_py)
            .arg("python")
            .arg(cfg.harness_py.join("ux_adapter.py"))
            .current_dir(&cfg.harness_py);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start ux adapter: {e}"))?;
        let stdin = child.stdin.take();
        let out = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
        let err = child
            .stderr
            .take()
            .map(|e| crate::proc::relay_stderr(e, "ux_critic"));
        let ready_s = spec.app.ready_timeout_s.unwrap_or(120.0);
        let stall = Duration::from_secs_f64(stall_s.max(1.0));
        // launching or relaunching an app waits for it to be ready, then the stall limit applies
        let slow = Duration::from_secs_f64(ready_s + stall_s);
        let mut a = ProcAdapter {
            child,
            stdin,
            out,
            err,
            stall,
            slow,
        };
        a.call(start, a.slow.max(Duration::from_secs(60))).await?;
        Ok(a)
    }

    async fn call(&mut self, req: Value, limit: Duration) -> Result<Value, String> {
        let op = req["op"].as_str().unwrap_or("?").to_string();
        let stdin = self.stdin.as_mut().ok_or("adapter closed")?;
        let line = format!("{req}\n");
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("adapter gone ({op}): {e}"))?;
        stdin.flush().await.map_err(|e| e.to_string())?;
        match tokio::time::timeout(limit, self.out.next_line()).await {
            Err(_) => Err(format!(
                "adapter stalled: no reply to {op} within {:.0}s",
                limit.as_secs_f64()
            )),
            Ok(Err(e)) => Err(format!("adapter output unreadable ({op}): {e}")),
            Ok(Ok(None)) => Err(format!(
                "adapter exited during {op}: {}",
                self.stderr_tail().await
            )),
            Ok(Ok(Some(l))) => {
                let v: Value = serde_json::from_str(&l)
                    .map_err(|e| format!("adapter reply to {op} is not JSON ({e}): {l:.200}"))?;
                if v["ok"] == true {
                    Ok(v)
                } else {
                    Err(format!("{op}: {}", v["error"].as_str().unwrap_or("failed")))
                }
            }
        }
    }

    async fn stderr_tail(&mut self) -> String {
        match self.err.take() {
            Some(h) => tokio::time::timeout(Duration::from_secs(3), h)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default()
                .chars()
                .rev()
                .take(400)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            None => String::new(),
        }
    }

    /// Ask the harness to close the app and exit; returns its closing report (desktop/unity:
    /// processes and ports the run left alive). Kills the process tree if it doesn't exit.
    pub async fn close(mut self) -> Option<Value> {
        let report = self
            .call(json!({"op": "close"}), Duration::from_secs(30))
            .await
            .ok();
        self.stdin = None;
        if tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .is_err()
        {
            kill_tree(&mut self.child);
        }
        report.and_then(|v| v.get("hygiene").cloned())
    }
}

/// The adapter for a spec: web runs in-process over CDP; desktop and Unity run the harness.
pub enum AppAdapter {
    Web(Box<crate::native::web::WebSession>),
    Proc(Box<ProcAdapter>),
}

impl AppAdapter {
    pub async fn launch(
        cfg: &Config,
        spec: &Spec,
        start: Value,
        stall_s: f64,
    ) -> Result<Self, String> {
        if spec.app.kind == "web" {
            crate::native::web::WebSession::start(cfg, &start)
                .await
                .map(|session| AppAdapter::Web(Box::new(session)))
        } else {
            ProcAdapter::launch(cfg, spec, start, stall_s)
                .await
                .map(|adapter| AppAdapter::Proc(Box::new(adapter)))
        }
    }

    /// Close the app; returns the harness's hygiene report, if it made one.
    pub async fn close(self) -> Option<Value> {
        match self {
            AppAdapter::Web(w) => {
                w.close().await;
                None
            }
            AppAdapter::Proc(p) => p.close().await,
        }
    }
}

impl Adapter for AppAdapter {
    async fn reset(&mut self) -> Result<(), String> {
        match self {
            AppAdapter::Web(w) => w.reset().await,
            AppAdapter::Proc(p) => p.reset().await,
        }
    }
    async fn observe(&mut self, opts: Value) -> Result<Obs, String> {
        match self {
            AppAdapter::Web(w) => w.observe(opts).await,
            AppAdapter::Proc(p) => p.observe(opts).await,
        }
    }
    async fn act(&mut self, id: &str) -> Result<(), String> {
        match self {
            AppAdapter::Web(w) => w.act(id).await,
            AppAdapter::Proc(p) => p.act(id).await,
        }
    }
}

fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        if cfg!(windows) {
            let _ = std::process::Command::new("taskkill")
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = child.start_kill();
    }
}

impl Drop for ProcAdapter {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            kill_tree(&mut self.child);
        }
    }
}

impl Adapter for ProcAdapter {
    async fn reset(&mut self) -> Result<(), String> {
        let limit = self.slow;
        self.call(json!({"op": "reset"}), limit).await.map(|_| ())
    }
    async fn observe(&mut self, opts: Value) -> Result<Obs, String> {
        let mut req = opts;
        req["op"] = json!("observe");
        let limit = self.stall;
        let v = self.call(req, limit).await?;
        serde_json::from_value(v["obs"].clone()).map_err(|e| format!("observation not valid: {e}"))
    }
    async fn act(&mut self, id: &str) -> Result<(), String> {
        let limit = self.stall;
        self.call(json!({"op": "act", "id": id}), limit)
            .await
            .map(|_| ())
    }
}

// ------------------------------------------------------------------ navigators

pub struct VisionNav<'a> {
    pub cfg: &'a Config,
}

impl Navigator for VisionNav<'_> {
    async fn choose(
        &self,
        goal: &str,
        pressed: &[String],
        image: Option<&str>,
        labels: &[String],
        seed: u64,
    ) -> Result<usize, String> {
        let image = image.ok_or("no marked screenshot for the vision novice")?;
        crate::vision::pick(self.cfg, Path::new(image), goal, pressed, labels, seed).await
    }
}

/// Text-only novice: Kev's p(yes) for spec `ux_novice_step` on each label, sampled in proportion.
pub struct KevNav<'a, D: Decider> {
    pub d: &'a D,
}

/// Deterministic sampling from weights with a seed (xorshift64*).
pub fn sample(weights: &[f64], seed: u64) -> usize {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    let u = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64;
    let total: f64 = weights.iter().map(|w| w.max(0.0)).sum();
    if total <= 0.0 {
        return (u * weights.len() as f64) as usize % weights.len().max(1);
    }
    let mut acc = 0.0;
    for (i, w) in weights.iter().enumerate() {
        acc += w.max(0.0) / total;
        if u < acc {
            return i;
        }
    }
    weights.len() - 1
}

impl<D: Decider> Navigator for KevNav<'_, D> {
    async fn choose(
        &self,
        goal: &str,
        pressed: &[String],
        _image: Option<&str>,
        labels: &[String],
        seed: u64,
    ) -> Result<usize, String> {
        let spec = crate::spec::get("ux_novice_step");
        let mut weights = Vec::with_capacity(labels.len());
        let mut goal: String = goal.chars().take(300).collect();
        if !pressed.is_empty() {
            let done: String = pressed
                .join(", ")
                .chars()
                .rev()
                .take(150)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            goal.push_str(&format!(" (already pressed: {done})"));
        }
        for l in labels {
            let state = spec.state(&[
                ("goal", goal.clone()),
                ("control", l.chars().take(120).collect()),
            ])?;
            let d = self.d.decide(&state, &spec.wire()).await?;
            weights.push(
                d.answers
                    .get("next_step")
                    .map(|a| a.value)
                    .ok_or("Kev gave no next_step answer")?,
            );
        }
        Ok(sample(&weights, seed))
    }
}

// ------------------------------------------------------------------ report types

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct GoalResult {
    pub name: String,
    /// Shortest number of actions from the home screen to a screen passing `done`; null when no
    /// explored screen passes it.
    pub optimal_steps: Option<usize>,
    /// Labels pressed on that shortest path.
    pub path: Vec<String>,
    /// Screenshot of the screen where `done` first held.
    pub screen: Option<usize>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct NoviceRun {
    pub seed: u64,
    pub success: bool,
    pub steps: usize,
    /// Labels it pressed, in order.
    pub tried: Vec<String>,
    /// The marked screenshots it saw.
    pub screenshots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct NoviceResult {
    pub model: String,
    pub runs: Vec<NoviceRun>,
    pub successes: usize,
    pub success_rate: f64,
    /// Successful runs that also stayed within the task's budget.
    pub within_budget: usize,
    /// Mean steps of the successful runs (compare with optimal_steps: the gap is searching).
    pub mean_steps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TaskResult {
    pub goal: String,
    pub primary: bool,
    pub budget: usize,
    #[serde(flatten)]
    pub best: GoalResult,
    pub within_budget: bool,
    /// Mean distance between successive controls on the optimal path, as a fraction of the
    /// screen diagonal (null for 1-step paths). Above 1/3 the workflow's controls are scattered.
    pub mean_step_distance: Option<f64>,
    pub novice: Option<NoviceResult>,
    /// Why this task fails; empty when it passes.
    pub fails: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ModeResult {
    #[serde(flatten)]
    pub best: GoalResult,
    /// Reached in at most one action from the home screen.
    pub top_level: bool,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RuleFinding {
    /// unlabelled | icon_no_tooltip | small_target | contrast | disabled_no_reason | scattered
    pub rule: String,
    /// error (fails approval) | warn
    pub severity: String,
    /// Control id (or axe target / task name).
    pub control: String,
    pub detail: String,
    /// Screens where it was seen.
    pub screens: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ScreenResult {
    pub id: usize,
    pub depth: usize,
    pub title: String,
    pub url: String,
    pub screenshot: Option<String>,
    pub controls: usize,
    /// How to get here from the home screen.
    pub path: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct Exploration {
    pub actions: usize,
    pub max_actions: usize,
    pub max_depth: usize,
    pub screens: usize,
    pub edges: usize,
    /// Why exploration stopped early (action cap, deadline), if it did.
    pub truncated: Option<String>,
    /// Actions that failed and replays that diverged (the app is not deterministic from reset).
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct UxResult {
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub spec: String,
    pub kind: String,
    pub purpose: String,
    /// false on any hard fail; null when the run could not complete (adapter error, or the
    /// novice was requested but unavailable).
    pub approval: Option<bool>,
    /// Approval covers declared functional tasks/rules, never visual craft.
    pub approval_scope: String,
    pub visual_quality_status: String,
    /// Missing top-level mode entries, unreachable / trivially-done / over-budget tasks, error-level
    /// rule findings, primary tasks the novice failed 2 of 3 times. Restructure, don't patch.
    pub hard_fails: Vec<String>,
    pub modes: Vec<ModeResult>,
    pub tasks: Vec<TaskResult>,
    pub rules: Vec<RuleFinding>,
    pub rule_counts: BTreeMap<String, usize>,
    /// Screens that were not the same screen when reached again, or were measured half-loaded, each
    /// with the two pictures to compare. The reviewer opens every pair.
    pub continuity: Vec<Continuity>,
    pub screens: Vec<ScreenResult>,
    pub exploration: Exploration,
    /// vision model, Kev spec, or "off"; with the reason when the novice did not run.
    pub novice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub novice_error: Option<String>,
    /// Processes and ports the run left alive (desktop/unity), reported then killed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hygiene: Option<Value>,
    pub out_dir: String,
    pub full_result: Option<String>,
    /// What to hand the `ux-critic` reviewer agent (~/.claude/agents/ux-critic.md).
    pub critic_brief: Value,
    pub duration_ms: u64,
}

// ------------------------------------------------------------------ engine

pub struct Options {
    pub max_depth: usize,
    pub max_actions: usize,
    pub runs: usize,
    pub deadline: Instant,
    pub out_dir: PathBuf,
}

struct Screen {
    fingerprint: String,
    depth: usize,
    path: Vec<Control>,
    title: String,
    url: String,
    screenshot: Option<String>,
    controls: usize,
    /// Each control's id and state (label, enabled, selected) and each visible text as first seen,
    /// so a repeat visit can be compared piece by piece.
    state: Vec<(String, String)>,
    texts: Vec<String>,
}

#[derive(Clone)]
struct Best {
    path: Vec<Control>,
    screen: usize,
}

/// One ux_critic run over one app. `explore` then (optionally, under the GPU lock) `novice`, then
/// `finish`.
pub struct Run<'s> {
    spec: &'s Spec,
    opts: Options,
    screens: Vec<Screen>,
    by_fp: HashMap<String, usize>,
    edges: usize,
    actions: usize,
    viewport: [f64; 2],
    /// modes first, then tasks
    best: Vec<Option<Best>>,
    rules: BTreeMap<(String, String), RuleFinding>,
    truncated: Option<String>,
    problems: Vec<String>,
    continuity: Vec<Continuity>,
    volatile: Vec<Regex>,
    /// Screens whose replay was compared with their first visit.
    expanded: usize,
    error: Option<String>,
    novice: Vec<Option<NoviceResult>>,
    novice_model: String,
    novice_gates: bool,
    novice_error: Option<String>,
    pub hygiene: Option<Value>,
    started: Instant,
}

fn path_labels(p: &[Control]) -> Vec<String> {
    p.iter().map(Control::shown).collect()
}

/// A screen's controls as (id, state), the state being what a person would notice changing.
fn screen_state(o: &Obs) -> Vec<(String, String)> {
    o.controls
        .iter()
        .map(|c| {
            (
                c.id.clone(),
                format!(
                    "{}|{}|{}",
                    c.label,
                    if c.enabled { "on" } else { "off" },
                    if c.selected { "selected" } else { "" }
                ),
            )
        })
        .collect()
}

/// Rule audit of one screen. Pure: (rule, severity, key, detail).
pub fn audit(
    controls: &[Control],
    axe: &[AxeViolation],
    viewport: [f64; 2],
) -> Vec<(&'static str, &'static str, String, String)> {
    const FIELDS: [&str; 6] = [
        "textbox", "combobox", "checkbox", "radio", "Edit", "ComboBox",
    ];
    let mut out = Vec::new();
    for c in controls {
        let no_text = c.text.trim().is_empty() && !FIELDS.contains(&c.role.as_str());
        if c.enabled && c.label.trim().is_empty() {
            out.push((
                "unlabelled",
                "error",
                c.id.clone(),
                format!("enabled {} with no accessible label", c.role),
            ));
        }
        if no_text && c.tooltip.trim().is_empty() {
            out.push((
                "icon_no_tooltip",
                "warn",
                c.id.clone(),
                format!("icon-only {} with no tooltip", c.role),
            ));
        }
        if !c.inline && (c.rect[2] < 24.0 || c.rect[3] < 24.0) {
            out.push((
                "small_target",
                "warn",
                c.id.clone(),
                format!("target {}x{} px, under 24x24", c.rect[2], c.rect[3]),
            ));
        }
        if !c.enabled && c.tooltip.trim().is_empty() {
            out.push((
                "disabled_no_reason",
                "warn",
                c.id.clone(),
                format!("disabled {} with no tooltip saying why", c.role),
            ));
        }
    }
    out.extend(layout_findings(controls, viewport));
    for v in axe.iter().filter(|v| v.id == "color-contrast") {
        for t in &v.targets {
            out.push(("contrast", "error", t.clone(), v.help.clone()));
        }
    }
    out
}

/// Controls drawn over one another, and controls drawn off the window. Both are invisible to a
/// reviewer reading labels and obvious to anyone looking: a button half under another, a label
/// clipped by the window edge.
fn layout_findings(
    controls: &[Control],
    viewport: [f64; 2],
) -> Vec<(&'static str, &'static str, String, String)> {
    const EDGE: f64 = 2.0;
    let mut out = Vec::new();
    let real: Vec<&Control> = controls
        .iter()
        .filter(|c| c.rect[2] > 0.0 && c.rect[3] > 0.0)
        .collect();
    if viewport[0] > 0.0 && viewport[1] > 0.0 {
        for c in &real {
            let [x, y, w, h] = c.rect;
            if x < -EDGE || y < -EDGE || x + w > viewport[0] + EDGE || y + h > viewport[1] + EDGE {
                out.push((
                    "offscreen",
                    "warn",
                    c.id.clone(),
                    format!("{} at {x:.0},{y:.0} ({w:.0}x{h:.0}) reaches outside the {:.0}x{:.0} window", c.shown(), viewport[0], viewport[1]),
                ));
            }
        }
    }
    for (i, a) in real.iter().enumerate() {
        for b in &real[i + 1..] {
            let (ax1, ay1) = (a.rect[0] + a.rect[2], a.rect[1] + a.rect[3]);
            let (bx1, by1) = (b.rect[0] + b.rect[2], b.rect[1] + b.rect[3]);
            let w = ax1.min(bx1) - a.rect[0].max(b.rect[0]);
            let h = ay1.min(by1) - a.rect[1].max(b.rect[1]);
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let smaller = (a.rect[2] * a.rect[3]).min(b.rect[2] * b.rect[3]);
            let share = w * h / smaller;
            // A control wholly inside another is a part of it, not a collision.
            let inside = |p: &Control, q: &Control| {
                p.rect[0] >= q.rect[0] - EDGE
                    && p.rect[1] >= q.rect[1] - EDGE
                    && p.rect[0] + p.rect[2] <= q.rect[0] + q.rect[2] + EDGE
                    && p.rect[1] + p.rect[3] <= q.rect[1] + q.rect[3] + EDGE
            };
            if share > 0.3 && !inside(a, b) && !inside(b, a) {
                out.push((
                    "overlap",
                    if share > 0.6 { "error" } else { "warn" },
                    format!("{} x {}", a.id, b.id),
                    format!(
                        "{} and {} overlap by {:.0}% of the smaller",
                        a.shown(),
                        b.shown(),
                        share * 100.0
                    ),
                ));
            }
        }
    }
    out
}

/// Mean distance between successive controls of a path, as a fraction of the screen diagonal.
pub fn mean_step_distance(path: &[Control], viewport: [f64; 2]) -> Option<f64> {
    let diag = (viewport[0].powi(2) + viewport[1].powi(2)).sqrt();
    if path.len() < 2 || diag <= 0.0 {
        return None;
    }
    let d: f64 = path
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0].centre(), w[1].centre());
            ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
        })
        .sum();
    Some(((d / (path.len() - 1) as f64 / diag) * 1000.0).round() / 1000.0)
}

/// Successive steps of one workflow more than a third of the screen apart on average: the
/// controls do not sit in one panel or toolbar.
pub const SCATTERED: f64 = 1.0 / 3.0;

impl<'s> Run<'s> {
    pub fn new(spec: &'s Spec, opts: Options) -> Self {
        let n = spec.modes.len() + spec.tasks.len();
        Run {
            spec,
            opts,
            screens: vec![],
            by_fp: HashMap::new(),
            edges: 0,
            actions: 0,
            viewport: [0.0, 0.0],
            best: vec![None; n],
            rules: BTreeMap::new(),
            truncated: None,
            problems: vec![],
            continuity: vec![],
            volatile: spec
                .volatile
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect(),
            expanded: 0,
            error: None,
            novice: vec![None; spec.tasks.len()],
            novice_model: "off".into(),
            novice_gates: false,
            novice_error: None,
            hygiene: None,
            started: Instant::now(),
        }
    }

    pub fn fail(&mut self, e: String) {
        self.error.get_or_insert(e);
    }

    fn goals(&self) -> impl Iterator<Item = &Goal> {
        self.spec.modes.iter().chain(self.spec.tasks.iter())
    }

    fn check_goals(&mut self, o: &Obs, path: &[Control], screen: usize) {
        let hits: Vec<usize> = self
            .goals()
            .enumerate()
            .filter(|(i, g)| self.best[*i].is_none() && g.done.holds(o))
            .map(|(i, _)| i)
            .collect();
        for i in hits {
            self.best[i] = Some(Best {
                path: path.to_vec(),
                screen,
            });
        }
    }

    fn add_screen(&mut self, o: Obs, depth: usize, path: Vec<Control>) -> usize {
        let id = self.screens.len();
        for (rule, sev, key, detail) in audit(&o.controls, &o.axe, o.viewport) {
            self.rules
                .entry((rule.to_string(), key.clone()))
                .or_insert_with(|| RuleFinding {
                    rule: rule.into(),
                    severity: sev.into(),
                    control: key,
                    detail,
                    screens: vec![],
                })
                .screens
                .push(id);
        }
        self.check_goals(&o, &path, id);
        self.by_fp.insert(o.fingerprint.clone(), id);
        if !o.busy.is_empty() {
            self.continuity.push(Continuity {
                kind: "seen_busy".into(),
                screen: id,
                path: path_labels(&path),
                detail: format!("still waiting for: {} (the wait ran out), so this screen was measured half-loaded", o.busy),
                added: vec![],
                removed: vec![],
                changed: vec![],
                before: o.screenshot.clone(),
                after: None,
                pixel_diff: None,
            });
        }
        let state = screen_state(&o);
        self.screens.push(Screen {
            fingerprint: o.fingerprint,
            depth,
            path,
            title: o.title,
            url: o.url,
            screenshot: o.screenshot,
            controls: o.controls.len(),
            state,
            texts: o.texts,
        });
        id
    }

    fn shot(&self, n: usize) -> Value {
        json!({"shot": self.opts.out_dir.join(format!("screen-{n:03}.png")), "axe": self.spec.app.kind == "web"})
    }

    /// Breadth-first over screens from the home screen, to `max_depth` and `max_actions`.
    pub async fn explore<A: Adapter>(&mut self, a: &mut A) {
        if let Err(e) = self.explore_inner(a).await {
            self.fail(e);
        }
    }

    async fn explore_inner<A: Adapter>(&mut self, a: &mut A) -> Result<(), String> {
        a.reset().await?;
        let home = a.observe(self.shot(0)).await?;
        self.viewport = home.viewport;
        self.add_screen(home, 0, vec![]);
        let mut queue = VecDeque::from([0usize]);
        'bfs: while let Some(s) = queue.pop_front() {
            if self.screens[s].depth >= self.opts.max_depth {
                continue;
            }
            let path = self.screens[s].path.clone();
            // the screen's controls, as they are on arrival
            a.reset().await?;
            replay(a, &path).await?;
            let here = a.observe(json!({})).await?;
            self.check_continuity(a, s, &here).await?;
            for c in here.actionable() {
                if self.actions >= self.opts.max_actions {
                    self.truncated = Some(format!("action cap {} reached", self.opts.max_actions));
                    break 'bfs;
                }
                if Instant::now() > self.opts.deadline {
                    self.truncated = Some("deadline reached".into());
                    break 'bfs;
                }
                a.reset().await?;
                if let Err(e) = replay(a, &path).await {
                    self.problems
                        .push(format!("replay to screen {s} failed: {e}"));
                    continue;
                }
                self.actions += 1;
                if let Err(e) = a.act(&c.id).await {
                    self.problems
                        .push(format!("pressing {} on screen {s} failed: {e}", c.shown()));
                    continue;
                }
                let o = a.observe(json!({})).await?;
                let mut p = path.clone();
                p.push(c.clone());
                self.edges += 1;
                match self.by_fp.get(&o.fingerprint) {
                    Some(&t) => self.check_goals(&o, &p, t),
                    None => {
                        let full = a.observe(self.shot(self.screens.len())).await?;
                        let fp = o.fingerprint.clone();
                        let t = self.add_screen(full, self.screens[s].depth + 1, p);
                        self.by_fp.insert(fp, t);
                        queue.push_back(t);
                    }
                }
            }
        }
        Ok(())
    }

    /// Compare a screen reached again with the first time it was reached: its controls and text piece
    /// by piece, and (when there is a first screenshot) the two pictures pixel by pixel. A difference
    /// means state leaked between visits: the app is not the same app each time it is reset, and
    /// every other measurement here is about whichever one it happened to be.
    async fn check_continuity<A: Adapter>(
        &mut self,
        a: &mut A,
        s: usize,
        here: &Obs,
    ) -> Result<(), String> {
        self.expanded += 1;
        let (added, removed, changed) = self.diff_against(s, here);
        let structural = !(added.is_empty() && removed.is_empty() && changed.is_empty());
        let first = self.screens[s].screenshot.clone();
        // The picture of the repeat, compared with the first, whether or not anything structural changed:
        // a half-drawn viewport or a leftover overlay is often invisible to the control tree.
        let mut after = None;
        let mut pixel_diff = None;
        if let Some(before) = &first {
            let shot = self.opts.out_dir.join(format!("screen-{s:03}.replay.png"));
            let o = a.observe(json!({"shot": shot, "compare": before})).await?;
            pixel_diff = o.pixel_diff;
            after = o.screenshot;
        }
        let visual = pixel_diff
            .as_ref()
            .is_some_and(|d| d.fraction > VISUAL_DIFF);
        let fingerprint_moved = here.fingerprint != self.screens[s].fingerprint;
        if !structural && !visual && !fingerprint_moved {
            if let Some(f) = &after {
                let _ = std::fs::remove_file(f);
            }
            return Ok(());
        }
        let kind = if structural {
            "replay_differs"
        } else {
            "looks_different"
        };
        let mut detail = if structural {
            format!(
                "reached again by the same path it looks different: {} added, {} removed, {} changed",
                added.len(),
                removed.len(),
                changed.len()
            )
        } else if visual {
            format!(
                "reached again by the same path its controls are the same but {:.0}% of the picture differs",
                pixel_diff.as_ref().map_or(0.0, |d| d.fraction) * 100.0
            )
        } else {
            "reached again by the same path it has a different fingerprint, but nothing that can be named differs".into()
        };
        if !here.busy.is_empty() {
            detail.push_str(&format!("; the app was still waiting for: {}", here.busy));
        }
        self.problems.push(format!("screen {s}: {detail}"));
        self.continuity.push(Continuity {
            kind: kind.into(),
            screen: s,
            path: path_labels(&self.screens[s].path),
            detail,
            added,
            removed,
            changed,
            before: first,
            after,
            pixel_diff,
        });
        Ok(())
    }

    /// What a repeat visit has that the first lacked, lacks that the first had, and has changed in
    /// place. Text the spec calls volatile is left out.
    fn diff_against(&self, s: usize, here: &Obs) -> (Vec<String>, Vec<String>, Vec<String>) {
        let before = &self.screens[s];
        let now = screen_state(here);
        let old: HashMap<&str, &str> = before
            .state
            .iter()
            .map(|(i, st)| (i.as_str(), st.as_str()))
            .collect();
        let new: HashMap<&str, &str> = now
            .iter()
            .map(|(i, st)| (i.as_str(), st.as_str()))
            .collect();
        let label = |id: &str| id.rsplit('/').next().unwrap_or(id).to_string();
        let mut added: Vec<String> = now
            .iter()
            .filter(|(i, _)| !old.contains_key(i.as_str()))
            .map(|(i, _)| format!("control {}", label(i)))
            .collect();
        let mut removed: Vec<String> = before
            .state
            .iter()
            .filter(|(i, _)| !new.contains_key(i.as_str()))
            .map(|(i, _)| format!("control {}", label(i)))
            .collect();
        let changed: Vec<String> = now
            .iter()
            .filter_map(|(i, st)| {
                old.get(i.as_str())
                    .filter(|o| **o != st.as_str())
                    .map(|o| format!("control {}: {} -> {}", label(i), o, st))
            })
            .collect();
        let stable = |t: &&String| !self.volatile.iter().any(|r| r.is_match(t));
        let old_text: std::collections::HashSet<&String> =
            before.texts.iter().filter(stable).collect();
        let new_text: std::collections::HashSet<&String> =
            here.texts.iter().filter(stable).collect();
        added.extend(
            new_text
                .difference(&old_text)
                .map(|t| format!("text \"{t}\"")),
        );
        removed.extend(
            old_text
                .difference(&new_text)
                .map(|t| format!("text \"{t}\"")),
        );
        for v in [&mut added, &mut removed] {
            v.sort();
            v.truncate(12);
        }
        (added, removed, changed.into_iter().take(12).collect())
    }

    /// Whether any task can be tried by the novice (reachable, and not already done at home).
    pub fn wants_novice(&self) -> bool {
        self.error.is_none()
            && self
                .spec
                .tasks
                .iter()
                .enumerate()
                .any(|(i, _)| self.task_best(i).is_some_and(|b| !b.path.is_empty()))
    }

    fn task_best(&self, t: usize) -> Option<&Best> {
        self.best[self.spec.modes.len() + t].as_ref()
    }

    /// Seeded novice runs for every task worth trying. `gates`: whether its failures may fail a
    /// primary task. Calibrated 2026-09-28 (README): the vision novice never failed a good-variant
    /// task; the Kev text novice failed two good variants 2 of 3 times, so it is advisory.
    pub async fn novice<A: Adapter, N: Navigator>(
        &mut self,
        a: &mut A,
        nav: &N,
        model: &str,
        gates: bool,
    ) {
        self.novice_model = if gates {
            model.into()
        } else {
            format!("{model}, advisory")
        };
        self.novice_gates = gates;
        for t in 0..self.spec.tasks.len() {
            if !self.task_best(t).is_some_and(|b| !b.path.is_empty()) {
                continue;
            }
            let mut runs = Vec::new();
            for seed in 1..=self.opts.runs as u64 {
                if Instant::now() > self.opts.deadline {
                    self.novice_error = Some("deadline reached during novice runs".into());
                    return;
                }
                match self.novice_run(a, nav, t, seed).await {
                    Ok(r) => runs.push(r),
                    Err(e) => {
                        self.novice_error = Some(e);
                        return;
                    }
                }
            }
            let ok: Vec<&NoviceRun> = runs.iter().filter(|r| r.success).collect();
            let successes = ok.len();
            let rate = (successes as f64 / runs.len().max(1) as f64 * 1000.0).round() / 1000.0;
            let within_budget = ok
                .iter()
                .filter(|r| r.steps <= self.spec.tasks[t].budget)
                .count();
            let mean_steps = (!ok.is_empty()).then(|| {
                (ok.iter().map(|r| r.steps as f64).sum::<f64>() / successes as f64 * 100.0).round()
                    / 100.0
            });
            self.novice[t] = Some(NoviceResult {
                model: model.into(),
                runs,
                successes,
                success_rate: rate,
                within_budget,
                mean_steps,
            });
        }
    }

    async fn novice_run<A: Adapter, N: Navigator>(
        &mut self,
        a: &mut A,
        nav: &N,
        t: usize,
        seed: u64,
    ) -> Result<NoviceRun, String> {
        let task = &self.spec.tasks[t];
        let dir = self.opts.out_dir.join("novice");
        let marks = |step: usize| json!({"marks": dir.join(format!("t{t}-r{seed}-s{step}.png"))});
        a.reset().await?;
        let mut o = a.observe(marks(0)).await?;
        let mut run = NoviceRun {
            seed,
            success: false,
            steps: 0,
            tried: vec![],
            screenshots: vec![],
        };
        for step in 0..task.budget + 3 {
            run.screenshots.extend(o.marked.clone());
            let options: Vec<Control> = o.actionable().into_iter().cloned().collect();
            if options.is_empty() {
                break;
            }
            let labels: Vec<String> = options.iter().map(Control::shown).collect();
            let i = nav
                .choose(
                    &task.goal,
                    &run.tried,
                    o.marked.as_deref(),
                    &labels,
                    seed * 1000 + step as u64,
                )
                .await?;
            run.tried.push(labels[i].clone());
            run.steps = step + 1;
            if let Err(e) = a.act(&options[i].id).await {
                self.problems
                    .push(format!("novice pressing {} failed: {e}", labels[i]));
            }
            o = a.observe(marks(step + 1)).await?;
            if task.done.holds(&o) {
                run.success = true;
                run.screenshots.extend(o.marked.clone());
                break;
            }
        }
        Ok(run)
    }

    pub fn set_novice_error(&mut self, model: &str, e: String) {
        self.novice_model = model.into();
        self.novice_error = Some(e);
    }

    fn goal_result(&self, i: usize, name: &str) -> GoalResult {
        let b = self.best[i].as_ref();
        GoalResult {
            name: name.into(),
            optimal_steps: b.map(|b| b.path.len()),
            path: b.map(|b| path_labels(&b.path)).unwrap_or_default(),
            screen: b.map(|b| b.screen),
        }
    }

    pub fn finish(mut self, novice_requested: bool) -> UxResult {
        let spec = self.spec;
        let depth = self.opts.max_depth;
        // A run that failed before exploring has measured nothing: say so rather than
        // reporting every mode and task as unreachable.
        let measured = self.error.is_none() && !self.screens.is_empty();
        let scope = match (&self.truncated, &self.error) {
            (_, Some(e)) if !measured => {
                format!("not measured: the run failed before exploring ({e})")
            }
            (_, Some(e)) => {
                format!("explored to depth {depth} until the run failed, so not exhaustively ({e})")
            }
            (Some(t), _) => format!("explored to depth {depth}, stopped early: {t}"),
            (None, _) => format!("explored exhaustively to depth {depth}"),
        };
        let mut hard = Vec::new();
        let modes: Vec<ModeResult> = spec
            .modes
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let best = self.goal_result(i, &m.name);
                let top = best.optimal_steps.is_some_and(|s| s <= 1);
                match best.optimal_steps {
                    None if !measured => {}
                    None => hard.push(format!(
                        "mode \"{}\": no screen passes its done-check ({scope})",
                        m.name
                    )),
                    Some(s) if s > 1 => hard.push(format!(
                        "mode \"{}\" has no top-level entry: {s} actions from the home screen ({})",
                        m.name,
                        best.path.join(" > ")
                    )),
                    _ => {}
                }
                ModeResult {
                    best,
                    top_level: top,
                }
            })
            .collect();
        let mut tasks = Vec::new();
        for (t, g) in spec.tasks.iter().enumerate() {
            let best = self.goal_result(spec.modes.len() + t, &g.name);
            let path = self
                .task_best(t)
                .map(|b| b.path.clone())
                .unwrap_or_default();
            let mut fails = Vec::new();
            match best.optimal_steps {
                None if !measured => fails.push(scope.clone()),
                None => fails.push(format!("unreachable: no screen passes its done-check ({scope})")),
                Some(0) => fails.push("its done-check already holds on the home screen, so it measures nothing: describe the screen the task ends on".into()),
                Some(s) if s > g.budget => fails.push(format!("takes {s} actions, budget {} ({})", g.budget, best.path.join(" > "))),
                _ => {}
            }
            let msd = mean_step_distance(&path, self.viewport);
            let at = self
                .task_best(t)
                .map(|b| vec![b.screen])
                .unwrap_or_default();
            if let Some(d) = msd.filter(|d| *d > SCATTERED) {
                self.rules.insert(("scattered".into(), g.name.clone()), RuleFinding {
                    rule: "scattered".into(),
                    severity: "warn".into(),
                    control: g.name.clone(),
                    detail: format!("successive controls of this task sit {:.0}% of the screen diagonal apart on average (grouped: under 33%)", d * 100.0),
                    screens: at,
                });
            }
            let novice = self.novice[t].clone();
            if g.primary
                && self.novice_gates
                && let Some(n) = &novice
            {
                let failed = n.runs.len() - n.successes;
                if !n.runs.is_empty() && failed * 3 >= n.runs.len() * 2 {
                    fails.push(format!(
                        "the novice failed {failed} of {} runs: the entry is not discoverable",
                        n.runs.len()
                    ));
                }
            }
            if measured {
                hard.extend(fails.iter().map(|f| {
                    format!(
                        "{}task \"{}\": {f}",
                        if g.primary { "primary " } else { "" },
                        g.name
                    )
                }));
            }
            tasks.push(TaskResult {
                goal: g.goal.clone(),
                primary: g.primary,
                budget: g.budget,
                within_budget: best.optimal_steps.is_some_and(|s| s >= 1 && s <= g.budget),
                best,
                mean_step_distance: msd,
                novice,
                fails,
            });
        }
        let rules: Vec<RuleFinding> = self.rules.into_values().collect();
        let mut counts = BTreeMap::new();
        for r in &rules {
            *counts.entry(r.rule.clone()).or_insert(0) += 1;
        }
        for (rule, n) in &counts {
            let errs: Vec<&str> = rules
                .iter()
                .filter(|r| &r.rule == rule && r.severity == "error")
                .map(|r| r.control.as_str())
                .collect();
            if !errs.is_empty() {
                hard.push(format!(
                    "rule {rule}: {n} finding(s), e.g. {}",
                    errs.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        let changed: Vec<&Continuity> = self
            .continuity
            .iter()
            .filter(|c| c.kind != "seen_busy")
            .collect();
        // The home screen differing from itself is the clearest sign that reset does not restore the start
        // state, so it fails alone; elsewhere it takes a tenth of the screens to call it a pattern.
        let home_changed = changed.iter().any(|c| c.screen == 0);
        if home_changed
            || (changed.len() >= 2 && changed.len() as f64 >= self.expanded as f64 * UNSTABLE_SHARE)
        {
            hard.push(format!(
                "the app is not the same from reset to reset: {} of {} screens changed between visits, e.g. screen {}: {}",
                changed.len(),
                self.expanded,
                changed[0].screen,
                changed[0].detail
            ));
        }
        if let Some(c) = self.continuity.iter().find(|c| c.kind == "seen_busy") {
            hard.push(format!(
                "the app was measured while still loading (screen {}): {}",
                c.screen, c.detail
            ));
        }
        let screens: Vec<ScreenResult> = self
            .screens
            .iter()
            .enumerate()
            .map(|(id, s)| ScreenResult {
                id,
                depth: s.depth,
                title: s.title.clone(),
                url: s.url.clone(),
                screenshot: s.screenshot.clone(),
                controls: s.controls,
                path: path_labels(&s.path),
            })
            .collect();
        let approval = if self.error.is_some() || self.screens.is_empty() {
            None
        } else if !hard.is_empty() {
            Some(false)
        } else if novice_requested && self.novice_error.is_some() {
            None
        } else {
            Some(true)
        };
        let out_dir = self.opts.out_dir.display().to_string();
        UxResult {
            tool: "ux_critic".into(),
            error: self.error,
            spec: spec.path.clone(),
            kind: spec.app.kind.clone(),
            purpose: spec.app.purpose.clone(),
            approval,
            approval_scope: "functional_tasks_and_rules_only".into(),
            visual_quality_status: "unassessed".into(),
            hard_fails: hard,
            modes,
            tasks,
            rule_counts: counts,
            rules,
            continuity: self.continuity,
            exploration: Exploration {
                actions: self.actions,
                max_actions: self.opts.max_actions,
                max_depth: depth,
                screens: screens.len(),
                edges: self.edges,
                truncated: self.truncated,
                problems: self.problems.into_iter().take(40).collect(),
            },
            screens,
            novice: self.novice_model,
            novice_error: self.novice_error,
            hygiene: self.hygiene,
            out_dir,
            full_result: None,
            critic_brief: Value::Null,
            duration_ms: self.started.elapsed().as_millis() as u64,
        }
    }
}

async fn replay<A: Adapter>(a: &mut A, path: &[Control]) -> Result<(), String> {
    for c in path {
        a.act(&c.id).await?;
    }
    Ok(())
}

/// What the `ux-critic` reviewer agent needs: the purpose, declared modes and tasks, the report,
/// and every screenshot to open. Called once `full_result` is known.
pub fn critic_brief(r: &UxResult, spec: &Spec) -> Value {
    let shots: Vec<&str> = r
        .screens
        .iter()
        .filter_map(|s| s.screenshot.as_deref())
        .take(40)
        .collect();
    let novice: Vec<&str> = r
        .tasks
        .iter()
        .filter_map(|t| t.novice.as_ref())
        .flat_map(|n| n.runs.iter().filter(|x| !x.success))
        .flat_map(|x| x.screenshots.iter().map(String::as_str))
        .take(20)
        .collect();
    let pairs: Vec<Value> = r
        .continuity
        .iter()
        .take(12)
        .map(|c| json!({"screen": c.screen, "kind": c.kind, "detail": c.detail, "before": c.before, "after": c.after, "pixel_diff": c.pixel_diff}))
        .collect();
    json!({
        "agent": "ux-critic",
        "agent_file": "~/.claude/agents/ux-critic.md",
        "approval_scope": "functional_tasks_and_rules_only",
        "visual_quality_status": "unassessed",
        "craft_reference_bank": "specs/visual-reference-anchors.md",
        "continuity_pairs": pairs,
        "purpose": spec.app.purpose,
        "modes": spec.modes.iter().map(|m| &m.name).collect::<Vec<_>>(),
        "tasks": spec.tasks.iter().map(|t| json!({"name": t.name, "goal": t.goal, "primary": t.primary, "budget": t.budget})).collect::<Vec<_>>(),
        "report": r.full_result,
        "screenshots": shots,
        "failed_novice_screenshots": novice,
        "prompt": format!(
            "Review this {} app as the ux-critic. Purpose: {} Read the ux_critic report at {} (rules, tasks with optimal_steps and novice traces, screens), then open every screenshot listed in it before judging. Functional approval does not establish professional visual quality. Independently assess typography, composition/density, purpose/identity, asset coherence and finishing detail against specs/visual-reference-anchors.md in the Kritic checkout. Inspect real genre-appropriate reference images before claiming a comparison. Competent generic is 7, professional 8, exceptional/showcase 9; every dimension needs named visible evidence. Do not require ornament or penalize intentional minimalism. Missing screenshots, viewports, interaction states or motion stay unverified; never certify AAA from a still. Report functional findings separately from verified craft observations and unresolved coverage. {}Hard fails: {}.",
            spec.app.kind, spec.app.purpose, r.full_result.as_deref().unwrap_or("(not saved)"),
            if r.continuity.is_empty() { String::new() } else { format!("Continuity: {} screen(s) were not the same when reached again; open each before/after pair in continuity_pairs side by side and say what a person would see change (layout shift, leftover or missing elements, a half-drawn area, a loading state, overlap), not only what the control list says. ", r.continuity.len()) },
            if r.hard_fails.is_empty() { "none".to_string() } else { r.hard_fails.join("; ") }),
    })
}

/// The `start` request for the adapter harness.
pub fn start_request(
    spec: &Spec,
    url: Option<&str>,
    allow_destructive: bool,
    allow_remote: bool,
    headed: bool,
    stall_s: f64,
    out_dir: &Path,
) -> Value {
    let a = &spec.app;
    json!({
        "op": "start", "kind": a.kind, "url": url.or(a.url.as_deref()), "viewport": a.viewport.map(|[w, h]| json!({"width": w, "height": h})),
        "exe": a.exe, "args": a.args, "cwd": a.cwd, "env": a.env, "window_title_re": a.window_title_re,
        "app_events": a.app_events, "ready_event": a.ready_event, "ready_timeout_s": a.ready_timeout_s.unwrap_or(120.0), "shot_dir": a.shot_dir,
        "settle_ms": a.settle_ms, "busy_timeout_s": a.busy_timeout_s.unwrap_or(45.0),
        "volatile": spec.volatile, "volatile_regions": spec.volatile_regions, "ignore": spec.ignore, "allow_destructive": allow_destructive, "allow_remote": allow_remote,
        "headed": headed, "stall_s": stall_s, "out_dir": out_dir,
    })
}

/// Default exploration depth: two beyond the largest budget, so an over-budget task is measured
/// rather than reported unreachable; at most 6.
pub fn default_depth(spec: &Spec) -> usize {
    (spec
        .tasks
        .iter()
        .map(|t| t.budget)
        .max()
        .unwrap_or(DEFAULT_BUDGET)
        + 2)
    .min(6)
}

//! Web UI testing over the Chrome DevTools Protocol, driving the Chrome or Edge already installed.
//!
//! Replaces the old Node + Playwright harness with no change to its reports: `ui_test` crawls
//! same-origin pages and exercises every visible control, `screenshot` captures a page for
//! `visual_critic`, and [`WebSession`] is the persistent session `ux_critic` explores. The in-page
//! logic is `web_page.js`; accessibility is axe-core, vendored in `assets/axe.min.js`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    MouseButton,
};
use chromiumoxide::cdp::browser_protocol::network::{
    EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent, EventResponseReceived,
};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, EventJavascriptDialogOpening, HandleJavaScriptDialogParams,
    NavigateParams,
};
use chromiumoxide::cdp::browser_protocol::target::{
    CloseTargetParams, CreateBrowserContextParams, CreateTargetParams, DisposeBrowserContextParams,
    EventTargetCreated,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    EvaluateParams, EventConsoleApiCalled, EventExceptionThrown,
};
use chromiumoxide::page::{Page, ScreenshotParams};
use futures_util::StreamExt;
use regex::Regex;
use serde_json::{Value, json};

const PAGE_JS: &str = include_str!("web_page.js");
const AXE_JS: &str = include_str!("../../assets/axe.min.js");

// Kept in parity with interactionState in harness/ui_test.mjs. Captured after point() scrolls
// the target into view, so tester preparation and the clicked button's ordinary focus are
// not confused with an effect produced by the application's action.
const INTERACTION_STATE_JS: &str = r#"(key) => {
  const target = document.querySelector(`[data-kritic-id="${key}"]`);
  const elements = [...document.querySelectorAll("*")];
  const active = document.activeElement;
  return {
    focus: elements.indexOf(active),
    redirected: !!active && active !== document.body && active !== document.documentElement && active !== target,
    scroll: [[-1, window.scrollX, window.scrollY], ...elements.flatMap((el, i) =>
      el.scrollHeight > el.clientHeight || el.scrollWidth > el.clientWidth
        ? [[i, el.scrollLeft, el.scrollTop]] : [])],
  };
}"#;

async fn interaction_state(page: &Page, key: &str) -> Option<Value> {
    eval(page, &format!("({INTERACTION_STATE_JS})({})", json!(key)))
        .await
        .ok()
}

fn interaction_changed(before: Option<&Value>, after: Option<&Value>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => {
            before["scroll"] != after["scroll"]
                || (after["redirected"] == true && before["focus"] != after["focus"])
        }
        _ => false,
    }
}

pub fn destructive() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)\b(delete|remove|destroy|drop|erase|wipe|reset all|log ?out|sign ?out|unsubscribe|purchase|buy|pay|checkout|deactivate|close account)\b").unwrap()
    })
}

/// Local development hosts, and file URLs.
pub fn is_local(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    if u.scheme() == "file" {
        return true;
    }
    let h = u.host_str().unwrap_or("").to_ascii_lowercase();
    matches!(h.as_str(), "localhost" | "127.0.0.1" | "[::1]" | "::1")
        || h.ends_with(".localhost")
        || h.ends_with(".test")
}

fn origin(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_default()
}

fn strip_hash(url: &str) -> String {
    url.split('#').next().unwrap_or(url).to_string()
}

fn test_value(ty: &str) -> &'static str {
    match ty {
        "email" => "kritic@example.test",
        "number" | "range" => "42",
        "url" => "https://example.test",
        "tel" => "5550100",
        "date" => "2026-01-01",
        "time" => "12:00",
        "datetime-local" => "2026-01-01T12:00",
        "month" => "2026-01",
        "week" => "2026-W01",
        "color" => "#336699",
        "password" => "kritic-test-pass",
        _ => "kritic test",
    }
}

/// The Chrome or Edge to drive: `ui.browser`, else an installed Chrome, Chromium or Edge, else
/// the Chrome for Testing that `kritic setup` downloads into `<home>/browsers`.
pub fn find_browser(cfg: &crate::config::Config) -> Result<PathBuf, String> {
    if let Some(b) = &cfg.browser {
        return if b.is_file() {
            Ok(b.clone())
        } else {
            Err(format!("ui.browser {} not found", b.display()))
        };
    }
    if let Ok(p) = chromiumoxide::detection::default_executable(
        chromiumoxide::detection::DetectionOptions::default(),
    ) {
        return Ok(p);
    }
    let local = cfg.home.join("browsers");
    let exe = if cfg!(windows) {
        "chrome-headless-shell.exe"
    } else {
        "chrome-headless-shell"
    };
    find_below(&local, exe, 3).ok_or_else(|| "no Chrome, Chromium or Edge found: install one, or run `kritic setup` to download Chrome for Testing".into())
}

fn find_below(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
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
        .find_map(|e| find_below(&e.path(), name, depth - 1))
}

// ------------------------------------------------------------------ browser

/// Events from one page, collected as they arrive.
#[derive(Default)]
struct Log {
    console_errors: Vec<Value>,
    page_errors: Vec<Value>,
    failed_requests: Vec<Value>,
    dialogs: Vec<Value>,
    popups: Vec<Value>,
    crashes: Vec<Value>,
    inflight: HashSet<String>,
    requests: u64,
    last_net: Option<Instant>,
    current: String,
}

type SharedLog = Arc<Mutex<Log>>;

/// A launched browser with its own throwaway profile, closed (and the profile removed) on drop.
pub struct Chrome {
    browser: Browser,
    handler: tokio::task::JoinHandle<()>,
    profile: tempfile::TempDir,
    pub version: String,
}

impl Chrome {
    pub async fn launch(
        cfg: &crate::config::Config,
        viewport: (u32, u32),
        headed: bool,
    ) -> Result<Chrome, String> {
        let exe = find_browser(cfg)?;
        let profile = tempfile::Builder::new()
            .prefix("kritic-browser-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let mut b = BrowserConfig::builder()
            .chrome_executable(&exe)
            .user_data_dir(profile.path())
            .window_size(viewport.0, viewport.1)
            .viewport(chromiumoxide::handler::viewport::Viewport {
                width: viewport.0,
                height: viewport.1,
                ..Default::default()
            })
            .launch_timeout(Duration::from_secs(30))
            .request_timeout(Duration::from_secs(30))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-extensions");
        if headed {
            b = b.with_head();
        } else {
            b = b.new_headless_mode();
        }
        if cfg!(target_os = "linux") {
            b = b.no_sandbox();
        }
        let conf = b.build().map_err(|e| format!("browser config: {e}"))?;
        let (browser, mut handler) = Browser::launch(conf)
            .await
            .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        let task = tokio::spawn(async move { while handler.next().await.is_some() {} });
        let version = browser
            .version()
            .await
            .map(|v| v.product)
            .unwrap_or_default();
        Ok(Chrome {
            browser,
            handler: task,
            profile,
            version,
        })
    }

    /// A page in a fresh browser context (no cookies or storage), with its event log wired.
    async fn page(
        &self,
        viewport: (u32, u32),
    ) -> Result<
        (
            Page,
            SharedLog,
            Option<chromiumoxide::cdp::browser_protocol::browser::BrowserContextId>,
        ),
        String,
    > {
        let ctx = self
            .browser
            .execute(CreateBrowserContextParams::default())
            .await
            .map_err(|e| e.to_string())?
            .result
            .browser_context_id;
        let page = self
            .browser
            .new_page(
                CreateTargetParams::builder()
                    .url("about:blank")
                    .browser_context_id(ctx.clone())
                    .build()
                    .map_err(|e| e.to_string())?,
            )
            .await
            .map_err(|e| format!("new page: {e}"))?;
        let _ = page
            .execute(chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams::new(viewport.0 as i64, viewport.1 as i64, 1.0, false))
            .await;
        let log: SharedLog = Arc::new(Mutex::new(Log::default()));
        wire(&self.browser, &page, log.clone()).await?;
        page.evaluate_on_new_document(PAGE_JS)
            .await
            .map_err(|e| e.to_string())?;
        Ok((page, log, Some(ctx)))
    }

    async fn dispose(
        &self,
        ctx: Option<chromiumoxide::cdp::browser_protocol::browser::BrowserContextId>,
    ) {
        if let Some(c) = ctx {
            let _ = self
                .browser
                .execute(DisposeBrowserContextParams::new(c))
                .await;
        }
    }

    pub async fn close(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(10), self.browser.close()).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), self.browser.wait()).await;
        let _ = self.browser.kill().await;
        self.handler.abort();
        let _ = self.profile.close();
    }
}

async fn wire(browser: &Browser, page: &Page, log: SharedLog) -> Result<(), String> {
    let e = |x: chromiumoxide::error::CdpError| x.to_string();
    let mut console = page
        .event_listener::<EventConsoleApiCalled>()
        .await
        .map_err(e)?;
    let mut exceptions = page
        .event_listener::<EventExceptionThrown>()
        .await
        .map_err(e)?;
    let mut sent = page
        .event_listener::<EventRequestWillBeSent>()
        .await
        .map_err(e)?;
    let mut done = page
        .event_listener::<EventLoadingFinished>()
        .await
        .map_err(e)?;
    let mut failed = page
        .event_listener::<EventLoadingFailed>()
        .await
        .map_err(e)?;
    let mut responses = page
        .event_listener::<EventResponseReceived>()
        .await
        .map_err(e)?;
    let mut dialogs = page
        .event_listener::<EventJavascriptDialogOpening>()
        .await
        .map_err(e)?;
    let mut crashes = page
        .event_listener::<chromiumoxide::cdp::browser_protocol::inspector::EventTargetCrashed>()
        .await
        .map_err(e)?;
    let mut targets = browser
        .event_listener::<EventTargetCreated>()
        .await
        .map_err(e)?;
    let me = page.target_id().clone();
    let (l1, l2, l3, l4, l5, l6, l7, l8, l9) = (
        log.clone(),
        log.clone(),
        log.clone(),
        log.clone(),
        log.clone(),
        log.clone(),
        log.clone(),
        log.clone(),
        log,
    );
    let p_dialog = page.clone();
    let p_popup = page.clone();
    tokio::spawn(async move {
        while let Some(ev) = console.next().await {
            if format!("{:?}", ev.r#type).to_lowercase().contains("error") {
                let text: Vec<String> = ev
                    .args
                    .iter()
                    .map(|a| {
                        a.value
                            .as_ref()
                            .map(|v| {
                                v.as_str()
                                    .map(String::from)
                                    .unwrap_or_else(|| v.to_string())
                            })
                            .or_else(|| a.description.clone())
                            .unwrap_or_default()
                    })
                    .collect();
                let mut l = l1.lock().unwrap();
                let page = l.current.clone();
                l.console_errors.push(json!({"page": page, "text": text.join(" ").chars().take(500).collect::<String>()}));
            }
        }
    });
    tokio::spawn(async move {
        while let Some(ev) = exceptions.next().await {
            let d = &ev.exception_details;
            let msg = d
                .exception
                .as_ref()
                .and_then(|x| x.description.clone())
                .unwrap_or_else(|| d.text.clone());
            let first = msg
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(500)
                .collect::<String>();
            let mut l = l2.lock().unwrap();
            let page = l.current.clone();
            l.page_errors.push(json!({"page": page, "message": first}));
        }
    });
    tokio::spawn(async move {
        while let Some(ev) = sent.next().await {
            let mut l = l3.lock().unwrap();
            l.inflight.insert(ev.request_id.inner().clone());
            l.requests += 1;
            l.last_net = Some(Instant::now());
        }
    });
    tokio::spawn(async move {
        while let Some(ev) = done.next().await {
            let mut l = l4.lock().unwrap();
            l.inflight.remove(ev.request_id.inner());
            l.last_net = Some(Instant::now());
        }
    });
    tokio::spawn(async move {
        while let Some(ev) = failed.next().await {
            let mut l = l5.lock().unwrap();
            l.inflight.remove(ev.request_id.inner());
            l.last_net = Some(Instant::now());
            if !ev.canceled.unwrap_or(false) {
                let page = l.current.clone();
                l.failed_requests.push(json!({"page": page, "request_id": ev.request_id.inner(), "failure": ev.error_text}));
            }
        }
    });
    tokio::spawn(async move {
        while let Some(ev) = responses.next().await {
            if ev.response.status >= 400 {
                let mut l = l6.lock().unwrap();
                let page = l.current.clone();
                l.failed_requests.push(
                    json!({"page": page, "url": ev.response.url, "status": ev.response.status}),
                );
            }
        }
    });
    tokio::spawn(async move {
        while let Some(ev) = dialogs.next().await {
            {
                let mut l = l7.lock().unwrap();
                let page = l.current.clone();
                l.dialogs.push(json!({"page": page, "type": format!("{:?}", ev.r#type).to_lowercase(), "message": ev.message.chars().take(200).collect::<String>()}));
            }
            let _ = p_dialog
                .execute(HandleJavaScriptDialogParams::new(false))
                .await;
        }
    });
    tokio::spawn(async move {
        while crashes.next().await.is_some() {
            let mut l = l8.lock().unwrap();
            let page = l.current.clone();
            l.crashes.push(json!({"page": page}));
        }
    });
    tokio::spawn(async move {
        while let Some(ev) = targets.next().await {
            let info = &ev.target_info;
            if info.opener_id.as_ref() == Some(&me) && info.r#type == "page" {
                {
                    let mut l = l9.lock().unwrap();
                    let page = l.current.clone();
                    l.popups.push(json!({"page": page, "url": info.url}));
                }
                let _ = p_popup
                    .execute(CloseTargetParams::new(info.target_id.clone()))
                    .await;
            }
        }
    });
    Ok(())
}

// ------------------------------------------------------------------ page helpers

async fn eval(page: &Page, expr: &str) -> Result<Value, String> {
    let p = EvaluateParams::builder()
        .expression(expr)
        .await_promise(true)
        .return_by_value(true)
        .build()
        .map_err(|e| e.to_string())?;
    let r = page
        .evaluate_expression(p)
        .await
        .map_err(|e| e.to_string())?;
    Ok(r.value().cloned().unwrap_or(Value::Null))
}

/// Call a `window.__kritic` helper with JSON arguments, installing the helpers first if the
/// document predates them.
async fn k(page: &Page, f: &str, args: &[Value]) -> Result<Value, String> {
    let a: Vec<String> = args.iter().map(|v| v.to_string()).collect();
    eval(
        page,
        &format!(
            "(() => {{ if (!window.__kritic) {{ {PAGE_JS} }} return window.__kritic.{f}({}); }})()",
            a.join(",")
        ),
    )
    .await
}

async fn goto(
    page: &Page,
    log: &SharedLog,
    url: &str,
    timeout: Duration,
) -> Result<Option<i64>, String> {
    log.lock().unwrap().current = url.to_string();
    tokio::time::timeout(timeout, page.goto(NavigateParams::new(url)))
        .await
        .map_err(|_| format!("navigation to {url} timed out"))?
        .map_err(|e| e.to_string())?;
    Ok(k(page, "status", &[]).await.ok().and_then(|v| v.as_i64()))
}

/// Wait `ms`, then until no request is in flight for `quiet_ms` (at most `cap`).
async fn settle(log: &SharedLog, ms: u64, quiet_ms: u64, cap: Duration) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
    let until = Instant::now() + cap;
    while Instant::now() < until {
        let (busy, last) = {
            let l = log.lock().unwrap();
            (!l.inflight.is_empty(), l.last_net)
        };
        if !busy && last.is_none_or(|t| t.elapsed() >= Duration::from_millis(quiet_ms)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn mouse(
    page: &Page,
    kind: DispatchMouseEventType,
    x: f64,
    y: f64,
    clicks: i64,
) -> Result<(), String> {
    let mut p = DispatchMouseEventParams::new(kind, x, y);
    p.button = Some(MouseButton::Left);
    p.click_count = Some(clicks);
    page.execute(p).await.map(|_| ()).map_err(|e| e.to_string())
}

/// A real mouse click at the element's centre (after scrolling it into view).
async fn click(page: &Page, key: &str, clicks: i64) -> Result<(), String> {
    let pt = k(page, "point", &[json!(key)]).await?;
    if pt.is_null() {
        return Err("element gone".into());
    }
    let (x, y) = (
        pt["x"].as_f64().unwrap_or(0.0),
        pt["y"].as_f64().unwrap_or(0.0),
    );
    mouse(page, DispatchMouseEventType::MouseMoved, x, y, 0).await?;
    for n in 1..=clicks {
        mouse(page, DispatchMouseEventType::MousePressed, x, y, n).await?;
        mouse(page, DispatchMouseEventType::MouseReleased, x, y, n).await?;
    }
    Ok(())
}

async fn hover(page: &Page, key: &str) -> Result<(), String> {
    let pt = k(page, "point", &[json!(key)]).await?;
    if pt.is_null() {
        return Err("element gone".into());
    }
    mouse(
        page,
        DispatchMouseEventType::MouseMoved,
        pt["x"].as_f64().unwrap_or(0.0),
        pt["y"].as_f64().unwrap_or(0.0),
        0,
    )
    .await
}

fn key_def(key: &str) -> (String, String, i64, Option<String>) {
    // (key, code, windowsVirtualKeyCode, text)
    match key {
        "Enter" => ("Enter".into(), "Enter".into(), 13, Some("\r".into())),
        "Tab" => ("Tab".into(), "Tab".into(), 9, None),
        "Escape" => ("Escape".into(), "Escape".into(), 27, None),
        "Backspace" => ("Backspace".into(), "Backspace".into(), 8, None),
        "Delete" => ("Delete".into(), "Delete".into(), 46, None),
        "Space" | " " => (" ".into(), "Space".into(), 32, Some(" ".into())),
        "ArrowUp" => ("ArrowUp".into(), "ArrowUp".into(), 38, None),
        "ArrowDown" => ("ArrowDown".into(), "ArrowDown".into(), 40, None),
        "ArrowLeft" => ("ArrowLeft".into(), "ArrowLeft".into(), 37, None),
        "ArrowRight" => ("ArrowRight".into(), "ArrowRight".into(), 39, None),
        "Home" => ("Home".into(), "Home".into(), 36, None),
        "End" => ("End".into(), "End".into(), 35, None),
        "PageUp" => ("PageUp".into(), "PageUp".into(), 33, None),
        "PageDown" => ("PageDown".into(), "PageDown".into(), 34, None),
        k if k.chars().count() == 1 => {
            let c = k.chars().next().unwrap();
            let code = if c.is_ascii_alphabetic() {
                format!("Key{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            (
                k.to_string(),
                code,
                c.to_ascii_uppercase() as i64,
                Some(k.to_string()),
            )
        }
        other => (other.to_string(), other.to_string(), 0, None),
    }
}

async fn press(page: &Page, key: &str) -> Result<(), String> {
    let (k_, code, vk, text) = key_def(key);
    let mut down = DispatchKeyEventParams::new(if text.is_some() {
        DispatchKeyEventType::KeyDown
    } else {
        DispatchKeyEventType::RawKeyDown
    });
    down.key = Some(k_.clone());
    down.code = Some(code.clone());
    down.windows_virtual_key_code = Some(vk);
    down.text = text;
    page.execute(down).await.map_err(|e| e.to_string())?;
    let mut up = DispatchKeyEventParams::new(DispatchKeyEventType::KeyUp);
    up.key = Some(k_);
    up.code = Some(code);
    up.windows_virtual_key_code = Some(vk);
    page.execute(up)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

async fn shot(page: &Page, path: &Path, full_page: bool) -> Result<(), String> {
    let bytes = page
        .screenshot(
            ScreenshotParams::builder()
                .format(CaptureScreenshotFormat::Png)
                .full_page(full_page)
                .build(),
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// axe-core violations: `rules` limits the run (ux_critic asks only for color-contrast).
async fn axe(page: &Page, rules: Option<&[&str]>) -> Result<Value, String> {
    let loaded = eval(page, "typeof window.axe === 'object'").await?;
    if loaded != Value::Bool(true) {
        eval(page, &format!("{AXE_JS}\n;true")).await?;
    }
    let opts = match rules {
        Some(r) => json!({"runOnly": {"type": "rule", "values": r}, "resultTypes": ["violations"]}),
        None => json!({"resultTypes": ["violations"]}),
    };
    let v = eval(
        page,
        &format!(
            "axe.run(document, {opts}).then(r => r.violations.map(v => ({{id: v.id, impact: v.impact, help: v.help, nodes: v.nodes.length, targets: v.nodes.slice(0, 3).map(n => n.target.join(' '))}})))"
        ),
    )
    .await?;
    Ok(v)
}

// ------------------------------------------------------------------ screenshot (visual_critic)

pub async fn screenshot(
    cfg: &crate::config::Config,
    url: &str,
    out_path: &Path,
    full_page: bool,
    allow_remote: bool,
) -> Value {
    if !allow_remote && !is_local(url) {
        return json!({"ok": false, "error": format!("refusing non-local URL {} (set allow_remote)", origin(url))});
    }
    let chrome = match Chrome::launch(cfg, (1280, 800), false).await {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let r = async {
        let (page, log, _) = chrome.page((1280, 800)).await?;
        let status = goto(&page, &log, url, Duration::from_secs(15)).await?;
        settle(&log, 300, 500, Duration::from_secs(2)).await;
        shot(&page, out_path, full_page).await?;
        let title = page.get_title().await.ok().flatten().unwrap_or_default();
        Ok::<_, String>(json!({"ok": true, "path": out_path, "status": status, "title": title}))
    }
    .await;
    chrome.close().await;
    r.unwrap_or_else(|e| json!({"ok": false, "error": e}))
}

// ------------------------------------------------------------------ ui_test

fn progress(event: &str, data: Value) {
    let mut d = data;
    d["event"] = json!(event);
    crate::events::emit("progress", json!({"tool": "ui_test", "data": d}));
}

pub async fn ui_test(cfg: &crate::config::Config, input: &Value) -> Value {
    let url = input["url"].as_str().unwrap_or_default().to_string();
    if !input["allow_remote"].as_bool().unwrap_or(false) && !is_local(&url) {
        return json!({"ok": false, "error": format!("refusing non-local URL {} (set allow_remote)", origin(&url))});
    }
    let vp = (
        input["viewport"]["width"].as_u64().unwrap_or(1280) as u32,
        input["viewport"]["height"].as_u64().unwrap_or(800) as u32,
    );
    let chrome = match Chrome::launch(cfg, vp, input["headed"].as_bool().unwrap_or(false)).await {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let version = chrome.version.clone();
    let r = run_ui_test(&chrome, input, &url, vp).await;
    chrome.close().await;
    match r {
        Ok(mut v) => {
            v["browser_version"] = json!(version);
            v
        }
        Err(e) => json!({"ok": false, "error": e}),
    }
}

async fn run_ui_test(
    chrome: &Chrome,
    input: &Value,
    start: &str,
    vp: (u32, u32),
) -> Result<Value, String> {
    let max_pages = input["max_pages"].as_u64().unwrap_or(5) as usize;
    let max_actions = input["max_actions"].as_u64().unwrap_or(60) as usize;
    let wait_ms = input["wait_ms"].as_u64().unwrap_or(250);
    let timeout = Duration::from_millis(input["timeout_ms"].as_u64().unwrap_or(10_000));
    let out_dir = PathBuf::from(input["out_dir"].as_str().unwrap_or("out"));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let allow_destructive = input["allow_destructive"].as_bool().unwrap_or(false);
    let start_origin = origin(start);
    let (mut page, mut log, mut ctx) = chrome.page(vp).await?;

    let mut pages: Vec<Value> = Vec::new();
    let mut interactions: Vec<Value> = Vec::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    if input["explore"].as_bool() != Some(false) {
        queue.push_back(start.to_string());
        for r in input["routes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Ok(u) = reqwest::Url::parse(start).and_then(|b| b.join(r)) {
                queue.push_back(u.to_string());
            }
        }
    }
    let mut seen = HashSet::new();
    let mut actions = 0usize;
    let mut storage_before: HashSet<String> = HashSet::new();
    let mut old_logs: Vec<SharedLog> = Vec::new();

    while let Some(url) = queue.pop_front() {
        if pages.len() >= max_pages {
            break;
        }
        let key = strip_hash(&url);
        if !seen.insert(key) {
            continue;
        }
        let t0 = Instant::now();
        let status = match goto(&page, &log, &url, timeout).await {
            Ok(s) => s,
            Err(e) => {
                pages.push(json!({"url": url, "status": null, "error": e.chars().take(300).collect::<String>()}));
                continue;
            }
        };
        settle(&log, wait_ms, 500, Duration::from_secs(2)).await;
        let mut rec = json!({"url": url, "status": status, "title": page.get_title().await.ok().flatten().unwrap_or_default(), "load_ms": t0.elapsed().as_millis() as u64});
        progress(
            "page",
            json!({"url": url, "status": status, "load_ms": rec["load_ms"], "n": pages.len() + 1}),
        );
        if pages.is_empty() {
            storage_before = k(&page, "storageKeys", &[])
                .await
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
        }
        if input["axe"].as_bool() != Some(false) {
            match axe(&page, None).await {
                Ok(v) => rec["axe"] = v,
                Err(e) => rec["axe_error"] = json!(e.chars().take(200).collect::<String>()),
            }
        }
        let shot_path = out_dir.join(format!("page-{}.png", pages.len()));
        if shot(&page, &shot_path, true).await.is_ok() {
            rec["screenshot"] = json!(shot_path);
        }
        pages.push(rec);
        for l in k(&page, "links", &[])
            .await
            .ok()
            .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
            .unwrap_or_default()
        {
            if origin(&l) == start_origin && (l.starts_with("http") || l.starts_with("file:")) {
                queue.push_back(strip_hash(&l));
            }
        }

        let cands: Vec<Value> = k(&page, "collect", &[json!({})])
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        for c in cands {
            if actions >= max_actions {
                break;
            }
            let (tag, ty, role, name) = (
                c["tag"].as_str().unwrap_or(""),
                c["type"].as_str().unwrap_or(""),
                c["role"].as_str().unwrap_or(""),
                c["name"].as_str().unwrap_or(""),
            );
            let href = c["href"].as_str().unwrap_or("");
            let mut rec = json!({"page": url, "element": {"tag": tag, "type": ty, "role": role, "name": name}, "action": "", "result": ""});
            let skip = |rec: &mut Value, why: &str| {
                rec["action"] = json!("none");
                rec["result"] = json!("skipped");
                rec["detail"] = json!(why);
            };
            if !allow_destructive
                && destructive().is_match(&format!(
                    "{name} {} {href}",
                    c["hint"].as_str().unwrap_or("")
                ))
            {
                skip(&mut rec, "looks destructive (set allow_destructive)");
                interactions.push(rec);
                continue;
            }
            if tag == "a"
                && !href.is_empty()
                && !href.starts_with("javascript:")
                && origin(href) != start_origin
            {
                skip(&mut rec, "external link");
                interactions.push(rec);
                continue;
            }
            if ty == "file" {
                skip(&mut rec, "file input");
                interactions.push(rec);
                continue;
            }
            let here = page.url().await.ok().flatten().unwrap_or_default();
            if strip_hash(&here) != strip_hash(&url) {
                let _ = goto(&page, &log, &url, timeout).await;
                settle(&log, wait_ms, 500, Duration::from_secs(2)).await;
                let _ = k(&page, "collect", &[json!({})]).await;
            }
            let id = c["kritic"].as_str().unwrap_or("").to_string();
            if k(&page, "point", &[json!(id)])
                .await
                .map(|v| v.is_null())
                .unwrap_or(true)
            {
                skip(&mut rec, "element gone after earlier interaction");
                interactions.push(rec);
                continue;
            }
            actions += 1;
            let (console_before, errors_before, crashes_before) = {
                let l = log.lock().unwrap();
                (l.console_errors.len(), l.page_errors.len(), l.crashes.len())
            };
            let before = k(&page, "signature", &[]).await.ok();
            let effect_before = interaction_state(&page, &id).await;
            let req_before = log.lock().unwrap().requests;
            let url_before = page.url().await.ok().flatten().unwrap_or_default();
            let action = if tag == "select" {
                "select"
            } else if tag == "textarea"
                || (tag == "input"
                    && !["checkbox", "radio", "submit", "button", "reset", "image"].contains(&ty))
            {
                "fill"
            } else if ty == "checkbox" || ty == "radio" || role == "checkbox" || role == "switch" {
                "toggle"
            } else {
                "click"
            };
            rec["action"] = json!(action);
            let acted = match action {
                "select" => {
                    let n = k(&page, "optionCount", &[json!(id)])
                        .await
                        .ok()
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    k(
                        &page,
                        "select",
                        &[json!(id), json!(if n > 1 { 1 } else { 0 })],
                    )
                    .await
                    .map(|_| ())
                }
                "fill" => match k(&page, "fill", &[json!(id), json!(test_value(ty))]).await {
                    Ok(_) => press(&page, "Tab").await,
                    Err(e) => Err(e),
                },
                _ => click(&page, &id, 1).await,
            };
            let acted = match acted {
                Ok(()) => tokio::time::timeout(
                    timeout,
                    settle(&log, wait_ms, 500, Duration::from_secs(2)),
                )
                .await
                .map_err(|_| "timed out".to_string()),
                Err(e) => Err(e),
            };
            match acted {
                Err(e) => {
                    rec["result"] = json!("error");
                    rec["errors"] = json!([e
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(300)
                        .collect::<String>()]);
                }
                Ok(()) => {
                    let after = k(&page, "signature", &[]).await.ok();
                    let effect_after = interaction_state(&page, &id).await;
                    let now = page.url().await.ok().flatten().unwrap_or_default();
                    let (new_errors, crashed, requests) = {
                        let l = log.lock().unwrap();
                        let mut errs: Vec<String> = l.page_errors[errors_before..]
                            .iter()
                            .map(|e| e["message"].as_str().unwrap_or("").to_string())
                            .collect();
                        errs.extend(
                            l.console_errors[console_before..]
                                .iter()
                                .map(|e| e["text"].as_str().unwrap_or("").to_string()),
                        );
                        (errs, l.crashes.len() > crashes_before, l.requests)
                    };
                    if crashed {
                        rec["result"] = json!("crash");
                        chrome.dispose(ctx.take()).await;
                        old_logs.push(log.clone());
                        (page, log, ctx) = chrome.page(vp).await?;
                    } else if !new_errors.is_empty() {
                        rec["result"] = json!("error");
                        rec["errors"] = json!(new_errors);
                    } else if now != url_before {
                        rec["result"] = json!("navigated");
                        rec["detail"] = json!(now);
                    } else if action == "click"
                        && before.is_some()
                        && before == after
                        && requests == req_before
                        && !interaction_changed(effect_before.as_ref(), effect_after.as_ref())
                    {
                        rec["result"] = json!("no_effect");
                    } else {
                        rec["result"] = json!("ok");
                    }
                }
            }
            progress(
                "interaction",
                json!({"n": actions, "name": name, "tag": tag, "action": action, "result": rec["result"]}),
            );
            interactions.push(rec);
        }
    }

    let flows = if input["flows"].as_array().is_some_and(|f| !f.is_empty()) {
        Some(run_flows(chrome, input, start, &out_dir, vp).await)
    } else {
        None
    };

    // persistence: set each selector, reload, compare
    let mut persistence = Vec::new();
    for p in input["persist"].as_array().into_iter().flatten() {
        let sel = p["selector"].as_str().unwrap_or("").to_string();
        let purl = p["url"]
            .as_str()
            .and_then(|u| reqwest::Url::parse(start).and_then(|b| b.join(u)).ok())
            .map(|u| u.to_string())
            .unwrap_or_else(|| start.to_string());
        let mut rec = json!({"selector": sel, "url": purl});
        let r = async {
            goto(&page, &log, &purl, timeout).await?;
            settle(&log, wait_ms, 500, Duration::from_secs(2)).await;
            let info = k(&page, "mark", &[json!(sel)]).await?;
            if info.is_null() {
                return Err(format!("no element matches {sel}"));
            }
            let kind = format!(
                "{}:{}",
                info["tag"].as_str().unwrap_or(""),
                info["type"].as_str().unwrap_or("")
            );
            let expected = if kind.ends_with(":checkbox") || kind.ends_with(":radio") {
                let was = k(&page, "value", &[json!("@target")]).await?;
                click(&page, "@target", 1).await?;
                json!(!was.as_bool().unwrap_or(false))
            } else if kind.starts_with("select") {
                let n = k(&page, "optionCount", &[json!("@target")])
                    .await?
                    .as_u64()
                    .unwrap_or(0);
                k(
                    &page,
                    "select",
                    &[json!("@target"), json!(n.saturating_sub(1))],
                )
                .await?
            } else {
                let v = format!(
                    "kritic-persist-{}",
                    chrono::Local::now().timestamp_millis() % 100_000
                );
                k(&page, "fill", &[json!("@target"), json!(v)]).await?;
                let _ = press(&page, "Tab").await;
                json!(v)
            };
            settle(&log, wait_ms, 500, Duration::from_secs(2)).await;
            page.reload().await.map_err(|e| e.to_string())?;
            settle(&log, wait_ms, 500, Duration::from_secs(2)).await;
            if k(&page, "mark", &[json!(sel)]).await?.is_null() {
                return Err(format!("{sel} is gone after reload"));
            }
            let actual = k(&page, "value", &[json!("@target")]).await?;
            Ok::<_, String>((expected, actual))
        }
        .await;
        match r {
            Ok((e, a)) => {
                rec["persisted"] = json!(e == a);
                rec["expected"] = e;
                rec["actual"] = a;
            }
            Err(e) => {
                rec["persisted"] = json!(false);
                rec["error"] = json!(e.chars().take(300).collect::<String>());
            }
        }
        persistence.push(rec);
    }

    let mut written = Vec::new();
    if goto(&page, &log, start, timeout).await.is_ok() {
        let keys: Vec<String> = k(&page, "storageKeys", &[])
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        written = keys
            .into_iter()
            .filter(|k| !storage_before.contains(k))
            .collect();
    }
    chrome.dispose(ctx).await;
    progress(
        "done",
        json!({"pages": pages.len(), "interactions": interactions.len()}),
    );

    // Merge every page log (a crash replaces the page and its log).
    old_logs.push(log);
    let mut merged = Log::default();
    for l in &old_logs {
        let l = l.lock().unwrap();
        merged
            .console_errors
            .extend(l.console_errors.iter().cloned());
        merged.page_errors.extend(l.page_errors.iter().cloned());
        merged
            .failed_requests
            .extend(l.failed_requests.iter().cloned());
        merged.dialogs.extend(l.dialogs.iter().cloned());
        merged.popups.extend(l.popups.iter().cloned());
        merged.crashes.extend(l.crashes.iter().cloned());
    }
    // One entry per URL; an HTTP status beats a network failure for the same request.
    let mut by_url: Vec<Value> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for f in merged.failed_requests {
        let key = f["url"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| f["request_id"].as_str().unwrap_or("").to_string());
        match index.get(&key) {
            Some(&i) => {
                let n = by_url[i]["count"].as_u64().unwrap_or(1) + 1;
                if by_url[i]["status"].is_null() && !f["status"].is_null() {
                    by_url[i] = f.clone();
                }
                by_url[i]["count"] = json!(n);
            }
            None => {
                let mut f = f;
                f["count"] = json!(1);
                index.insert(key, by_url.len());
                by_url.push(f);
            }
        }
    }
    // Network failures have no URL of their own; drop the ones an HTTP status already explains.
    by_url.retain(|f| {
        !(f["url"].is_null() && f["failure"].as_str().is_some_and(|s| s.contains("ABORTED")))
    });
    let mut out = json!({
        "ok": true, "url": start, "pages": pages, "interactions": interactions,
        "console_errors": merged.console_errors, "page_errors": merged.page_errors, "failed_requests": by_url,
        "crashes": merged.crashes, "dialogs": merged.dialogs, "popups": merged.popups,
        "persistence": persistence, "storage_keys_written": written,
    });
    if let Some(f) = flows {
        out["flows"] = f;
    }
    Ok(out)
}

// ------------------------------------------------------------------ flows

async fn run_step(
    page: &Page,
    log: &SharedLog,
    step: &Value,
    base: &str,
    out_dir: &Path,
    shot_name: &str,
) -> Result<Option<PathBuf>, String> {
    let t = Duration::from_millis(step["timeout_ms"].as_u64().unwrap_or(5000));
    let target = step["target"].as_str();
    let val = step["value"].as_str().map(String::from).unwrap_or_else(|| {
        if step["value"].is_null() {
            String::new()
        } else {
            step["value"].to_string()
        }
    });
    let need = || {
        target.ok_or_else(|| {
            format!(
                "step \"{}\" needs a target",
                step["do"].as_str().unwrap_or("?")
            )
        })
    };
    // Poll until the target resolves (and is visible, when asked), like an auto-waiting locator.
    let find = |want_visible: bool| async move {
        let sel = need()?;
        let until = Instant::now() + t;
        loop {
            let info = k(page, "mark", &[json!(sel)]).await?;
            if !info.is_null() && (!want_visible || info["visible"] == true) {
                return Ok::<_, String>(info);
            }
            if Instant::now() >= until {
                return Err(format!(
                    "{sel}: no {}match within {:.0}s",
                    if want_visible { "visible " } else { "" },
                    t.as_secs_f64()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    match step["do"].as_str().unwrap_or("") {
        "navigate" => {
            let u = reqwest::Url::parse(base)
                .and_then(|b| b.join(&val))
                .map_err(|e| e.to_string())?;
            goto(page, log, u.as_str(), t).await?;
        }
        "click" => {
            find(true).await?;
            click(page, "@target", 1).await?;
        }
        "dblclick" => {
            find(true).await?;
            click(page, "@target", 2).await?;
        }
        "fill" => {
            find(true).await?;
            k(page, "fill", &[json!("@target"), json!(val)]).await?;
        }
        "select" => {
            find(true).await?;
            k(page, "select", &[json!("@target"), json!(val)]).await?;
        }
        "check" | "uncheck" => {
            find(true).await?;
            let want = step["do"] == "check";
            if k(page, "value", &[json!("@target")]).await?.as_bool() != Some(want) {
                click(page, "@target", 1).await?;
            }
        }
        "press" => {
            if target.is_some() {
                find(true).await?;
                eval(
                    page,
                    "document.querySelector('[data-kritic-target]')?.focus()",
                )
                .await?;
            }
            press(page, &val).await?;
        }
        "hover" => {
            find(true).await?;
            hover(page, "@target").await?;
        }
        "wait_for" | "expect_visible" => {
            find(true).await?;
        }
        "expect_hidden" => {
            let sel = need()?;
            let until = Instant::now() + t;
            loop {
                let info = k(page, "mark", &[json!(sel)]).await?;
                if info.is_null() || info["visible"] != true {
                    break;
                }
                if Instant::now() >= until {
                    return Err(format!("{sel} still visible after {:.0}s", t.as_secs_f64()));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        "wait" => tokio::time::sleep(Duration::from_millis(val.parse().unwrap_or(500))).await,
        "expect_text" => {
            let until = Instant::now() + t;
            let mut text = String::new();
            while Instant::now() < until {
                if !k(page, "mark", &[json!(need()?)]).await?.is_null() {
                    text = k(page, "textOf", &[json!("@target")])
                        .await?
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    if text.is_empty() {
                        text = k(page, "value", &[json!("@target")])
                            .await?
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                    }
                    if text.contains(&val) {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if !text.contains(&val) {
                return Err(format!(
                    "expected text containing \"{val}\", got \"{}\"",
                    text.chars().take(120).collect::<String>()
                ));
            }
        }
        "expect_url" => {
            let u = page.url().await.ok().flatten().unwrap_or_default();
            if !u.contains(&val) {
                return Err(format!("expected URL containing \"{val}\", got {u}"));
            }
        }
        "screenshot" => {
            let p = out_dir.join(format!("{shot_name}.png"));
            shot(page, &p, true).await?;
            return Ok(Some(p));
        }
        other => return Err(format!("unknown step \"{other}\"")),
    }
    Ok(None)
}

async fn run_flows(
    chrome: &Chrome,
    input: &Value,
    start: &str,
    out_dir: &Path,
    vp: (u32, u32),
) -> Value {
    let mut results = Vec::new();
    for (fi, flow) in input["flows"].as_array().into_iter().flatten().enumerate() {
        let name = flow["name"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| format!("flow {}", fi + 1));
        let steps = flow["steps"].as_array().cloned().unwrap_or_default();
        progress("flow", json!({"name": name, "steps": steps.len()}));
        let mut fr = json!({"name": name, "passed": true, "steps": []});
        let (page, log, ctx) = match chrome.page(vp).await {
            Ok(x) => x,
            Err(e) => {
                fr["passed"] = json!(false);
                fr["steps"] = json!([{"i": 0, "do": "open", "result": "fail", "error": e}]);
                results.push(fr);
                continue;
            }
        };
        let open = flow["url"]
            .as_str()
            .and_then(|u| reqwest::Url::parse(start).and_then(|b| b.join(u)).ok())
            .map(|u| u.to_string())
            .unwrap_or_else(|| start.to_string());
        let mut passed = true;
        let mut recs = Vec::new();
        if let Err(e) = goto(&page, &log, &open, Duration::from_secs(15)).await {
            passed = false;
            recs.push(json!({"i": 0, "do": "open", "result": "fail", "error": e}));
        }
        let cont = flow["continue_on_error"].as_bool().unwrap_or(false);
        for (i, step) in steps.iter().enumerate() {
            let mut rec =
                json!({"i": i + 1, "do": step["do"], "target": step["target"], "result": "ok"});
            if !passed && !cont {
                rec["result"] = json!("skipped");
                recs.push(rec);
                continue;
            }
            let (e0, c0, d0) = {
                let l = log.lock().unwrap();
                (l.page_errors.len(), l.console_errors.len(), l.dialogs.len())
            };
            let t0 = Instant::now();
            match run_step(
                &page,
                &log,
                step,
                start,
                out_dir,
                &format!("flow{}-step{}", fi + 1, i + 1),
            )
            .await
            {
                Ok(p) => {
                    if let Some(p) = p {
                        rec["screenshot"] = json!(p);
                    }
                    settle(&log, 0, 500, Duration::from_millis(1500)).await;
                }
                Err(e) => {
                    rec["result"] = json!("fail");
                    rec["error"] = json!(
                        e.lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(300)
                            .collect::<String>()
                    );
                }
            }
            let new_errs: Vec<String> = {
                let l = log.lock().unwrap();
                let mut v: Vec<String> = l.page_errors[e0..]
                    .iter()
                    .map(|e| format!("page error: {}", e["message"].as_str().unwrap_or("")))
                    .collect();
                v.extend(
                    l.console_errors[c0..]
                        .iter()
                        .map(|e| format!("console: {}", e["text"].as_str().unwrap_or(""))),
                );
                v.extend(l.dialogs[d0..].iter().map(|e| {
                    format!(
                        "dialog {}: {}",
                        e["type"].as_str().unwrap_or(""),
                        e["message"].as_str().unwrap_or("")
                    )
                }));
                v
            };
            if !new_errs.is_empty() && rec["result"] == "ok" {
                rec["result"] = json!("fail");
                rec["error"] = json!(new_errs.join(" | "));
            } else if !new_errs.is_empty() {
                rec["errors"] = json!(new_errs);
            }
            rec["ms"] = json!(t0.elapsed().as_millis() as u64);
            if rec["result"] == "fail" {
                passed = false;
                let p = out_dir.join(format!("flow{}-fail{}.png", fi + 1, i + 1));
                if shot(&page, &p, true).await.is_ok() {
                    rec["screenshot"] = json!(p);
                }
            }
            progress(
                "flow_step",
                json!({"flow": name, "i": i + 1, "do": step["do"], "result": rec["result"]}),
            );
            recs.push(rec);
        }
        fr["passed"] = json!(passed);
        fr["steps"] = json!(recs);
        results.push(fr);
        chrome.dispose(ctx).await;
    }
    json!(results)
}

// ------------------------------------------------------------------ ux_critic session

/// The persistent browser session `ux_critic` explores: reset gives a fresh context on the
/// start URL; controls get stable ids (role, label and the index among controls sharing both).
pub struct WebSession {
    chrome: Chrome,
    page: Option<(
        Page,
        SharedLog,
        Option<chromiumoxide::cdp::browser_protocol::browser::BrowserContextId>,
    )>,
    url: String,
    viewport: (u32, u32),
    settle_ms: u64,
    ignore: Vec<Regex>,
    allow_destructive: bool,
    current: HashMap<String, String>,
}

fn role_of(c: &Value) -> String {
    let (role, tag, ty) = (
        c["role"].as_str().unwrap_or(""),
        c["tag"].as_str().unwrap_or(""),
        c["type"].as_str().unwrap_or(""),
    );
    if !role.is_empty() {
        return role.into();
    }
    match tag {
        "a" => "link".into(),
        "select" => "combobox".into(),
        "textarea" => "textbox".into(),
        "summary" => "button".into(),
        "input" if ty == "checkbox" || ty == "radio" => ty.into(),
        "input" if ["submit", "button", "reset", "image"].contains(&ty) => "button".into(),
        "input" => "textbox".into(),
        t => t.into(),
    }
}

impl WebSession {
    pub async fn start(cfg: &crate::config::Config, start: &Value) -> Result<WebSession, String> {
        let url = start["url"]
            .as_str()
            .ok_or("web ux spec has no url")?
            .to_string();
        if !start["allow_remote"].as_bool().unwrap_or(false) && !is_local(&url) {
            return Err(format!(
                "refusing non-local URL {} (set allow_remote)",
                origin(&url)
            ));
        }
        let viewport = (
            start["viewport"]["width"].as_u64().unwrap_or(1280) as u32,
            start["viewport"]["height"].as_u64().unwrap_or(800) as u32,
        );
        let chrome =
            Chrome::launch(cfg, viewport, start["headed"].as_bool().unwrap_or(false)).await?;
        let ignore = start["ignore"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|r| Regex::new(r).ok())
            .collect();
        Ok(WebSession {
            chrome,
            page: None,
            url,
            viewport,
            settle_ms: start["settle_ms"].as_u64().unwrap_or(150),
            ignore,
            allow_destructive: start["allow_destructive"].as_bool().unwrap_or(false),
            current: HashMap::new(),
        })
    }

    fn page(
        &self,
    ) -> Result<
        &(
            Page,
            SharedLog,
            Option<chromiumoxide::cdp::browser_protocol::browser::BrowserContextId>,
        ),
        String,
    > {
        self.page.as_ref().ok_or_else(|| "reset first".into())
    }

    async fn map_controls(&mut self) -> Result<Vec<Value>, String> {
        let (page, _, _) = self.page()?;
        let raw: Vec<Value> =
            serde_json::from_value(k(page, "collect", &[json!({"all": true})]).await?)
                .map_err(|e| e.to_string())?;
        let start_origin = origin(&self.url);
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut current = HashMap::new();
        let controls = raw
            .iter()
            .map(|c| {
                let role = role_of(c);
                let label = c["label"].as_str().unwrap_or("");
                let key = format!("{role}:{}", if label.is_empty() { c["text"].as_str().unwrap_or("") } else { label });
                let n = seen.entry(key.clone()).or_insert(0);
                *n += 1;
                let id = format!("{key}#{n}");
                current.insert(id.clone(), c["kritic"].as_str().unwrap_or("").to_string());
                let mut out = json!({"id": id, "label": c["label"], "text": c["text"], "tooltip": c["tooltip"], "role": role, "rect": c["rect"],
                    "enabled": c["enabled"], "inline": c["inline"], "selected": c["selected"]});
                let href = c["href"].as_str().unwrap_or("");
                if self.ignore.iter().any(|r| r.is_match(label)) {
                    out["skip"] = json!("ignored");
                } else if !self.allow_destructive && destructive().is_match(&format!("{label} {} {href}", c["hint"].as_str().unwrap_or(""))) {
                    out["skip"] = json!("destructive");
                } else if c["type"] == "file" {
                    out["skip"] = json!("file input");
                } else if !href.is_empty() && !href.starts_with("javascript:") && origin(href) != start_origin {
                    out["skip"] = json!("external link");
                }
                out
            })
            .collect();
        self.current = current;
        Ok(controls)
    }

    pub async fn close(self) {
        if let Some((_, _, ctx)) = self.page {
            self.chrome.dispose(ctx).await;
        }
        self.chrome.close().await;
    }
}

fn fingerprint(url: &str, controls: &[Value], text: &str) -> String {
    use sha2::{Digest, Sha256};
    let u = reqwest::Url::parse(url)
        .map(|u| {
            format!(
                "{}{}",
                u.path(),
                u.fragment().map(|f| format!("#{f}")).unwrap_or_default()
            )
        })
        .unwrap_or_default();
    let digits = Regex::new(r"\d+").unwrap();
    let mut sig = vec![u];
    sig.extend(controls.iter().map(|c| {
        format!(
            "{}|{}|{}",
            c["id"].as_str().unwrap_or(""),
            c["enabled"],
            c["selected"]
        )
    }));
    sig.push(digits.replace_all(text, "0").into_owned());
    let h = Sha256::digest(sig.join("\n").as_bytes());
    h.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

impl crate::ux::Adapter for WebSession {
    async fn reset(&mut self) -> Result<(), String> {
        if let Some((_, _, ctx)) = self.page.take() {
            self.chrome.dispose(ctx).await;
        }
        let (page, log, ctx) = self.chrome.page(self.viewport).await?;
        goto(&page, &log, &self.url, Duration::from_secs(30)).await?;
        settle(&log, self.settle_ms, 50, Duration::from_secs(3)).await;
        self.page = Some((page, log, ctx));
        self.current.clear();
        Ok(())
    }

    async fn observe(&mut self, opts: Value) -> Result<crate::ux::Obs, String> {
        let controls = self.map_controls().await?;
        let (page, _, _) = self.page()?;
        let text = k(page, "text", &[])
            .await?
            .as_str()
            .unwrap_or("")
            .to_string();
        let url = page.url().await.ok().flatten().unwrap_or_default();
        let collapsed = Regex::new(r"\s+")
            .unwrap()
            .replace_all(&text, " ")
            .chars()
            .take(20_000)
            .collect::<String>();
        let mut obs = json!({
            "fingerprint": fingerprint(&url, &controls, &text), "url": url,
            "title": page.get_title().await.ok().flatten().unwrap_or_default(),
            "text": collapsed, "viewport": [self.viewport.0, self.viewport.1], "controls": controls,
        });
        if opts["axe"] == true {
            match axe(page, Some(&["color-contrast"])).await {
                Ok(v) => obs["axe"] = v,
                Err(e) => obs["axe_error"] = json!(e.chars().take(200).collect::<String>()),
            }
        }
        if let Some(p) = opts["shot"].as_str() {
            shot(page, Path::new(p), false).await?;
            obs["screenshot"] = json!(p);
        }
        if let Some(p) = opts["marks"].as_str() {
            // numbered in the order kritic lists the actionable controls to the novice
            let marks: Vec<Value> = obs["controls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["enabled"] == true && c.get("skip").is_none())
                .enumerate()
                .map(|(i, c)| json!({"n": i + 1, "x": c["rect"][0], "y": c["rect"][1], "w": c["rect"][2], "h": c["rect"][3]}))
                .collect();
            k(page, "drawMarks", &[json!(marks)]).await?;
            let r = shot(page, Path::new(p), false).await;
            k(page, "clearMarks", &[]).await?;
            r?;
            obs["marked"] = json!(p);
        }
        serde_json::from_value(obs).map_err(|e| format!("observation: {e}"))
    }

    async fn act(&mut self, id: &str) -> Result<(), String> {
        self.map_controls().await?; // replays act without observing in between
        let key = self
            .current
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no control {id} on this screen"))?;
        let (page, log, _) = self.page()?;
        let info = k(page, "point", &[json!(key)]).await?;
        let (tag, ty) = (
            info["tag"].as_str().unwrap_or(""),
            info["type"].as_str().unwrap_or(""),
        );
        if tag == "select" {
            let n = k(page, "optionCount", &[json!(key)])
                .await?
                .as_u64()
                .unwrap_or(0);
            k(
                page,
                "select",
                &[json!(key), json!(if n > 1 { 1 } else { 0 })],
            )
            .await?;
        } else if tag == "textarea"
            || (tag == "input"
                && !["checkbox", "radio", "submit", "button", "reset", "image"].contains(&ty))
        {
            k(page, "fill", &[json!(key), json!("kritic test")]).await?;
            let _ = press(page, "Tab").await;
        } else {
            click(page, &key, 1).await?;
        }
        settle(log, self.settle_ms, 50, Duration::from_secs(3)).await;
        Ok(())
    }
}

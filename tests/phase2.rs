//! Phase 2: ui_test judging + vision parsing (offline), and live end-to-end runs against the
//! harness fixtures (KRITIC_LIVE=1: needs node + Playwright Chromium; visual tests need Ollama).
//!   cargo test --test phase2
//!   KRITIC_LIVE=1 cargo test --release --test phase2 -- --nocapture --test-threads=1

use kritic::config::Config;
use kritic::ui::{HarnessReport, UiOptions, judge};
use kritic::vision;
use serde_json::{Value, json};

fn report(v: Value) -> HarnessReport {
    serde_json::from_value(v).unwrap()
}

fn opts() -> UiOptions {
    UiOptions { axe_blocking: true, ignore: vec![] }
}

// ------------------------------------------------------------------ ui judge (offline)

#[test]
fn clean_report_approves() {
    let r = report(json!({
        "ok": true,
        "pages": [{"url": "http://localhost/", "status": 200, "axe": [{"id": "region", "impact": "moderate", "help": "x", "nodes": 1}]}],
        "interactions": [
            {"page": "p", "element": {"tag": "button", "name": "Save"}, "action": "click", "result": "ok"},
            {"page": "p", "element": {"tag": "a", "name": "About"}, "action": "click", "result": "navigated"},
            {"page": "p", "element": {"tag": "button", "name": "Delete"}, "action": "none", "result": "skipped"}
        ],
        "failed_requests": [{"url": "http://localhost/favicon.ico", "status": 404}],
        "persistence": [{"selector": "#a", "persisted": true}]
    }));
    let (s, b, approval, _) = judge(report_clone(&r), &opts());
    assert_eq!(approval, Some(true), "{b:?}");
    assert_eq!((s.ok, s.navigated, s.skipped, s.axe_serious), (1, 1, 1, 0));
}

fn report_clone(r: &HarnessReport) -> HarnessReport {
    r.clone()
}

#[test]
fn every_breakage_kind_rejects() {
    let cases = [
        json!({"ok": true, "page_errors": [{"message": "x is not defined"}]}),
        json!({"ok": true, "console_errors": [{"text": "boom"}]}),
        json!({"ok": true, "failed_requests": [{"url": "http://l/api", "status": 500}]}),
        json!({"ok": true, "crashes": [{"page": "p"}]}),
        json!({"ok": true, "persistence": [{"selector": "#n", "persisted": false, "expected": "a", "actual": ""}]}),
        json!({"ok": true, "interactions": [{"page": "p", "element": {"name": "Go"}, "action": "click", "result": "no_effect"}]}),
        json!({"ok": true, "interactions": [{"page": "p", "element": {"name": "Go"}, "action": "click", "result": "error", "errors": ["e"]}]}),
        json!({"ok": true, "pages": [{"url": "u", "status": 500}]}),
        json!({"ok": true, "pages": [{"url": "u", "status": 200, "axe": [{"id": "color-contrast", "impact": "serious", "help": "h", "nodes": 2}]}]}),
    ];
    for c in cases {
        let (_, b, approval, _) = judge(report(c.clone()), &opts());
        assert_eq!(approval, Some(false), "{c}");
        assert_eq!(b.len(), 1, "{c} -> {b:?}");
    }
}

#[test]
fn ignore_list_and_non_blocking_axe() {
    let r = json!({"ok": true,
        "interactions": [{"page": "p", "element": {"name": "Tooltip trigger"}, "action": "click", "result": "no_effect"}],
        "pages": [{"url": "u", "status": 200, "axe": [{"id": "color-contrast", "impact": "serious", "help": "h", "nodes": 2}]}]});
    let (s, b, approval, _) = judge(report(r), &UiOptions { axe_blocking: false, ignore: vec!["Tooltip".into()] });
    assert_eq!(approval, Some(true), "{b:?}");
    assert_eq!(s.axe_serious, 1);
}

#[test]
fn harness_failure_is_unknown_not_pass() {
    let (_, _, approval, _) = judge(report(json!({"ok": false, "error": "refusing non-local URL"})), &opts());
    assert_eq!(approval, None);
}

#[test]
fn resource_404_console_line_is_not_double_counted() {
    let r = json!({"ok": true,
        "console_errors": [{"text": "Failed to load resource: the server responded with a status of 404 (Not Found)"}],
        "failed_requests": [{"url": "http://l/x.png", "status": 404}]});
    let (_, b, _, _) = judge(report(r), &opts());
    assert_eq!(b.len(), 1, "{b:?}");
}

// ------------------------------------------------------------------ vision parsing (offline)

fn vjson(score: f64) -> Value {
    json!({"layout_clarity": score, "spacing_alignment": score, "visual_hierarchy": score, "color_contrast": score,
           "accessibility": score, "spacing_issues": ["a"], "contrast_issues": [], "accessibility_issues": ["b"]})
}

#[test]
fn vision_output_is_validated() {
    assert!(vision::parse(&vjson(7.0)).is_ok());
    assert!(vision::parse(&vjson(11.0)).is_err());
    let mut missing = vjson(7.0);
    missing.as_object_mut().unwrap().remove("color_contrast");
    assert!(vision::parse(&missing).is_err());
    let mut no_list = vjson(7.0);
    no_list.as_object_mut().unwrap().remove("spacing_issues");
    assert!(vision::parse(&no_list).is_err());
}

#[test]
fn vision_approval_is_min_score_threshold() {
    let mut r = vision::VisualResult::default();
    let (s, obs) = vision::parse(&vjson(7.0)).unwrap();
    vision::judge(&mut r, s, obs, 6.0);
    assert_eq!(r.approval, Some(true));
    assert_eq!(r.unverified_observations, vec!["a", "b"]);
    let mut low = vjson(7.0);
    low["color_contrast"] = json!(3);
    let mut r = vision::VisualResult::default();
    let (s, obs) = vision::parse(&low).unwrap();
    vision::judge(&mut r, s, obs, 6.0);
    assert_eq!(r.approval, Some(false));
    assert_eq!(r.reasons, vec!["color_contrast 3/10 < 6"]);
}

// ------------------------------------------------------------------ live

fn live() -> bool {
    std::env::var("KRITIC_LIVE").as_deref() == Ok("1")
}

/// Minimal static server for the harness fixtures; returns the base URL.
async fn serve_fixtures() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/harness/test/fixtures"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let dir = dir.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/").split(['?', '#']).next().unwrap_or("/").to_string();
                let file = dir.join(path.trim_start_matches('/'));
                let (status, body) = match std::fs::read(&file) {
                    Ok(b) if file.starts_with(&dir) && file.is_file() => ("200 OK", b),
                    _ => ("404 Not Found", b"not found".to_vec()),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
            });
        }
    });
    format!("http://127.0.0.1:{}", addr.port())
}

#[tokio::test]
async fn live_ui_test_good_vs_broken() {
    if !live() {
        return;
    }
    let cfg = Config::from_env();
    let base = serve_fixtures().await;
    let persist_good = vec![json!({"selector": "#name"}), json!({"selector": "#dark"}), json!({"selector": "#size"})];
    let good = kritic::ui::ui_test(&cfg, &format!("{base}/good.html"), &[], &persist_good, 5, 60, false, false, false, true, &[], true, opts()).await;
    eprintln!("good: approval={:?} summary={:?} breakages={:?} err={:?} {}ms", good.approval, good.summary, good.breakages, good.error, good.duration_ms);
    assert_eq!(good.approval, Some(true), "{:?}", good.breakages);

    let persist_bad = vec![json!({"selector": "#note"})];
    let bad = kritic::ui::ui_test(&cfg, &format!("{base}/broken.html"), &[], &persist_bad, 5, 60, false, false, false, true, &[], true, opts()).await;
    eprintln!("broken: approval={:?} summary={:?}", bad.approval, bad.summary);
    for b in &bad.breakages {
        eprintln!("  - {b}");
    }
    assert_eq!(bad.approval, Some(false));
    let all = bad.breakages.join("\n");
    for needle in ["doesNotExist", "handler failed", "no observable effect", "api/missing", "logo.png", "did not persist", "image-alt", "color-contrast"] {
        assert!(all.contains(needle), "missing `{needle}` in breakages");
    }
}

#[tokio::test]
async fn live_visual_polished_vs_messy() {
    if !live() {
        return;
    }
    let cfg = Config::from_env();
    let base = serve_fixtures().await;
    for (page, expect) in [("polished.html", true), ("messy.html", false)] {
        let out = vision::run_dir(&cfg, "test").join(format!("{page}.png"));
        kritic::ui::screenshot(&cfg, &format!("{base}/{page}"), &out, false, false).await.unwrap();
        let r = vision::critique(&cfg, &out, "").await;
        eprintln!("{page}: approval={:?} scores={:?} obs={:?} err={:?} stopped_laya={} {}ms", r.approval, r.scores, r.unverified_observations, r.error, r.stopped_laya, r.latency_ms);
        assert!(r.model_ok, "{:?}", r.error);
        assert_eq!(r.approval, Some(expect), "{page}");
    }
}

#[test]
fn control_errors_are_not_repeated_as_page_errors() {
    let r = json!({"ok": true,
        "interactions": [{"page": "p", "element": {"tag": "button", "name": "Throw"}, "action": "click", "result": "error", "errors": ["x is not defined"]}],
        "page_errors": [{"message": "x is not defined"}, {"message": "other"}]});
    let (_, b, _, _) = judge(report(r), &opts());
    assert_eq!(b.len(), 2, "{b:?}");
    assert!(b.iter().any(|x| x.contains("other")));
}

#[test]
fn filler_observations_are_dropped() {
    let mut v = vjson(8.0);
    v["spacing_issues"] = json!(["No issues", "none", "Save button misaligned"]);
    let (_, obs) = vision::parse(&v).unwrap();
    assert_eq!(obs, vec!["Save button misaligned", "b"]);
}

#[test]
fn run_dirs_are_unique_even_in_the_same_millisecond() {
    let cfg = Config::from_env();
    let dirs: std::collections::HashSet<_> = (0..50).map(|_| vision::run_dir(&cfg, "visual")).collect();
    assert_eq!(dirs.len(), 50);
}

#[test]
fn failed_flows_become_breakages_naming_the_step() {
    let flows = vec![
        json!({"name": "ok", "passed": true, "steps": [{"i": 1, "do": "click", "result": "ok"}]}),
        json!({"name": "save", "passed": false, "steps": [
            {"i": 1, "do": "fill", "result": "ok"},
            {"i": 2, "do": "click", "target": "text=Save", "result": "fail", "error": "timeout"},
            {"i": 3, "do": "expect_text", "result": "skipped"}]}),
    ];
    let b = kritic::ui::judge_flows(&flows);
    assert_eq!(b.len(), 1);
    assert!(b[0].contains("\"save\" failed at step 2 (click") && b[0].contains("timeout"), "{}", b[0]);
}

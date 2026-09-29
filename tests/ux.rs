//! ux_critic calibration against the contrastive web fixtures (harness/test/fixtures/ux): the same
//! app built well and badly. Live: needs node + Playwright Chromium; the novice needs Ollama
//! (vision) or laya (Kev).
//!   KRITIC_LIVE=1 cargo test --release --test ux -- --nocapture --test-threads=1
//!   KRITIC_UX_NOVICE=vision|kev|off (default vision), KRITIC_UX_PAIRS=modes,labels (default all four)
//! Results are written to calibration/ux-<date>.json.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kritic::config::Config;
use kritic::kev::KevClient;
use kritic::ux::{self, KevNav, Options, ProcAdapter, Run, UxResult, VisionNav};
use serde_json::json;

fn live() -> bool {
    std::env::var("KRITIC_LIVE").as_deref() == Ok("1")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("harness/test/fixtures/ux")
}

fn file_url(p: &Path) -> String {
    let s = std::fs::canonicalize(p).unwrap().display().to_string();
    format!("file:///{}", s.trim_start_matches(r"\\?\").replace('\\', "/"))
}

async fn run(cfg: &Config, pair: &str, variant: &str, novice: &str) -> UxResult {
    let spec = ux::load_spec(&fixtures().join(format!("{pair}.toml"))).unwrap();
    let out_dir = kritic::vision::run_dir(cfg, &format!("ux-cal-{pair}-{variant}"));
    let opts = Options {
        max_depth: ux::default_depth(&spec),
        max_actions: 150,
        runs: 3,
        deadline: Instant::now() + Duration::from_secs(900),
        out_dir: out_dir.clone(),
    };
    let url = file_url(&fixtures().join(format!("{pair}-{variant}.html")));
    let start = ux::start_request(&spec, Some(&url), false, false, false, 15.0, &out_dir);
    let mut r = Run::new(&spec, opts);
    let mut a = ProcAdapter::launch(cfg, &spec, start, 15.0).await.expect("adapter");
    r.explore(&mut a).await;
    if novice != "off" && r.wants_novice() {
        if novice == "vision" {
            kritic::gpu::stop_laya(cfg).await;
            r.novice(&mut a, &VisionNav { cfg }, &cfg.vision_model, true).await;
        } else {
            kritic::gpu::unload_vision(cfg).await;
            let kev = KevClient::new(std::sync::Arc::new(cfg.clone()));
            r.novice(&mut a, &KevNav { d: &kev }, "kev-4b", false).await;
        }
    }
    a.close().await;
    r.finish(novice != "off")
}

fn novice_rate(r: &UxResult) -> (usize, usize) {
    r.tasks.iter().filter_map(|t| t.novice.as_ref()).fold((0, 0), |(s, n), x| (s + x.successes, n + x.runs.len()))
}

fn rules(r: &UxResult, rule: &str) -> usize {
    r.rule_counts.get(rule).copied().unwrap_or(0)
}

#[tokio::test]
async fn calibrate_fixture_pairs() {
    if !live() {
        eprintln!("skipped (set KRITIC_LIVE=1)");
        return;
    }
    let cfg = Config::from_env();
    let novice = std::env::var("KRITIC_UX_NOVICE").unwrap_or_else(|_| "vision".into());
    let pairs = std::env::var("KRITIC_UX_PAIRS").unwrap_or_else(|_| "modes,labels,depth,group".into());
    let mut table = Vec::new();
    for pair in pairs.split(',') {
        let good = run(&cfg, pair, "good", &novice).await;
        let bad = run(&cfg, pair, "bad", &novice).await;
        for r in [&good, &bad] {
            assert!(r.error.is_none(), "{pair}: {:?}", r.error);
            assert!(r.exploration.truncated.is_none(), "{pair}: exploration truncated: {:?}", r.exploration);
        }
        let row = |r: &UxResult| {
            let (s, n) = novice_rate(r);
            json!({
                "approval": r.approval, "hard_fails": r.hard_fails, "rule_counts": r.rule_counts,
                "optimal_steps": r.tasks.iter().map(|t| (t.best.name.clone(), t.best.optimal_steps)).collect::<Vec<_>>(),
                "mode_steps": r.modes.iter().map(|m| (m.best.name.clone(), m.best.optimal_steps)).collect::<Vec<_>>(),
                "mean_step_distance": r.tasks.iter().map(|t| t.mean_step_distance).collect::<Vec<_>>(),
                "novice": {"successes": s, "runs": n,
                           "within_budget": r.tasks.iter().filter_map(|t| t.novice.as_ref()).map(|x| x.within_budget).sum::<usize>(),
                           "mean_steps": r.tasks.iter().map(|t| (t.best.name.clone(), t.novice.as_ref().and_then(|x| x.mean_steps))).collect::<Vec<_>>(), "rate": if n > 0 { s as f64 / n as f64 } else { f64::NAN },
                           "per_task": r.tasks.iter().map(|t| (t.best.name.clone(), t.novice.as_ref().map(|x| x.successes))).collect::<Vec<_>>(),
                           "tried": r.tasks.iter().map(|t| (t.best.name.clone(), t.novice.as_ref().map(|x| x.runs.iter().map(|r| r.tried.join(" > ")).collect::<Vec<_>>()))).collect::<Vec<_>>()},
                "novice_error": r.novice_error, "screens": r.exploration.screens, "actions": r.exploration.actions,
                "duration_ms": r.duration_ms, "out_dir": r.out_dir,
            })
        };
        let (g, b) = (row(&good), row(&bad));
        eprintln!("{pair}\n  good {g}\n  bad  {b}");
        table.push(json!({"pair": pair, "good": g, "bad": b}));
        // deterministic separation, pair by pair (independent of the novice)
        let det = |r: &UxResult| r.hard_fails.iter().filter(|h| !h.contains("novice")).count();
        assert_eq!(det(&good), 0, "{pair} good has deterministic hard fails: {:?}", good.hard_fails);
        match pair {
            "modes" => {
                assert!(bad.hard_fails.iter().any(|h| h.contains("mode \"Composer\" has no top-level entry")), "{:?}", bad.hard_fails);
                assert!(bad.tasks.iter().any(|t| t.best.name == "select a section" && !t.within_budget));
            }
            "labels" => {
                assert_eq!((rules(&good, "unlabelled"), rules(&good, "icon_no_tooltip")), (0, 0));
                assert!(rules(&bad, "unlabelled") >= 9 && rules(&bad, "icon_no_tooltip") >= 9, "{:?}", bad.rule_counts);
                assert_eq!(bad.approval, Some(false));
            }
            "depth" => {
                assert_eq!(good.tasks[0].best.optimal_steps, Some(1));
                assert_eq!(bad.tasks[0].best.optimal_steps, Some(5));
                assert!(bad.hard_fails.iter().any(|h| h.contains("takes 5 actions, budget 3")), "{:?}", bad.hard_fails);
            }
            "group" => {
                assert_eq!(rules(&good, "scattered"), 0);
                assert_eq!(rules(&bad, "scattered"), 1, "{:?}", bad.rules);
                assert!(bad.tasks[0].mean_step_distance.unwrap() > ux::SCATTERED && good.tasks[0].mean_step_distance.unwrap() < ux::SCATTERED);
            }
            _ => unreachable!(),
        }
    }
    let out = json!({"date": chrono::Local::now().format("%Y-%m-%d").to_string(), "novice": novice, "pairs": table});
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("calibration/ux-{}-{novice}-{}.json", chrono::Local::now().format("%Y-%m-%d"), pairs.replace(',', "-")));
    std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    eprintln!("written to {}", path.display());
}

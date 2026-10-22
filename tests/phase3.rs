//! Phase 3: analyzer parsers, metrics, approval rules (offline) and live runs of the real
//! scanners, desktop tester and resource sampler (KRITIC_LIVE=1).
//!   cargo test --test phase3
//!   KRITIC_LIVE=1 cargo test --release --test phase3 -- --nocapture --test-threads=1

use std::collections::BTreeMap;
use std::path::Path;

use kritic::config::Config;
use kritic::kev::{Answer, Decider, Decision, KevClient, Question};
use kritic::metrics::{duplicates, functions};
use kritic::phase3::{self, Budgets, QualityOptions, check_budgets, judge_desktop, security_score};
use kritic::scan::{self, cvss_to_severity};
use serde_json::{Value, json};

// ------------------------------------------------------------------ parsers

#[test]
fn parses_bandit() {
    let j = r#"{"results":[{"test_id":"B602","test_name":"subprocess_popen_with_shell_equals_true","issue_severity":"HIGH","issue_confidence":"HIGH","filename":"./app.py","line_number":10,"issue_text":"subprocess call with shell=True"}]}"#;
    let f = scan::parse_bandit(Path::new("."), j).unwrap();
    assert_eq!(
        (f[0].severity.as_str(), f[0].file.as_str(), f[0].line),
        ("high", "app.py", 10)
    );
}

#[test]
fn parses_semgrep_severity_mapping() {
    let j = r#"{"results":[{"check_id":"python.lang.security.subprocess-shell-true","path":"app.py","start":{"line":3},"extra":{"severity":"ERROR","message":"shell=True"}},
                           {"check_id":"x.y.warn","path":"b.py","start":{"line":1},"extra":{"severity":"WARNING","message":"m"}}]}"#;
    let f = scan::parse_semgrep(Path::new("."), j).unwrap();
    assert_eq!(f[0].rule, "subprocess-shell-true");
    assert_eq!(
        (f[0].severity.as_str(), f[1].severity.as_str()),
        ("high", "medium")
    );
}

#[test]
fn parses_gitleaks_and_redacts() {
    let j = r#"[{"RuleID":"aws-access-token","File":"app.py","StartLine":5,"Description":"AWS key","Secret":"AKIA...","Match":"AKIA..."}]"#;
    let f = scan::parse_gitleaks(Path::new("."), j).unwrap();
    assert_eq!(f[0].severity, "high");
    assert!(!f[0].message.contains("AKIA"), "secret must not be echoed");
    assert!(scan::parse_gitleaks(Path::new("."), "").unwrap().is_empty());
}

#[test]
fn parses_osv_one_finding_per_package_at_worst_cvss() {
    let j = r#"{"results":[{"source":{"path":"C:/p/requirements.txt"},"packages":[
        {"package":{"name":"jinja2","version":"2.10"},"vulnerabilities":[{"id":"PYSEC-1"},{"id":"PYSEC-2"}],"groups":[{"max_severity":"5.4"},{"max_severity":"8.6"}]},
        {"package":{"name":"ok","version":"1"},"vulnerabilities":[],"groups":[]}]}]}"#;
    let f = scan::parse_osv(j).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].severity.as_str(), f[0].file.as_str()),
        ("high", "requirements.txt")
    );
    assert!(f[0].message.contains("2 security advisories"));
    assert_eq!(cvss_to_severity(9.8), "critical");
    assert_eq!(cvss_to_severity(0.0), "info");
}

#[test]
fn parses_ruff_clippy_eslint_tsc() {
    let base = Path::new(".");
    let ruff = r#"[{"code":"F821","filename":"a.py","location":{"row":3},"message":"Undefined name `x`"},{"code":"E501","filename":"a.py","location":{"row":4},"message":"long"}]"#;
    let f = scan::parse_ruff(base, ruff).unwrap();
    assert_eq!(
        (f[0].severity.as_str(), f[1].severity.as_str()),
        ("high", "low")
    );
    let clippy = "{\"reason\":\"compiler-message\",\"message\":{\"level\":\"warning\",\"message\":\"needless borrow\",\"code\":{\"code\":\"clippy::needless_borrow\"},\"spans\":[{\"is_primary\":true,\"file_name\":\"src/a.rs\",\"line_start\":7}]}}\n{\"reason\":\"build-finished\"}";
    let c = scan::parse_clippy(base, clippy);
    assert_eq!((c.len(), c[0].line, c[0].severity.as_str()), (1, 7, "low"));
    let eslint = r#"[{"filePath":"a.js","messages":[{"ruleId":"no-undef","severity":2,"line":1,"message":"x is not defined"}]}]"#;
    assert_eq!(
        scan::parse_eslint(base, eslint).unwrap()[0].severity,
        "high"
    );
    let tsc = "src/a.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.";
    let t = scan::parse_tsc(base, tsc);
    assert_eq!((t[0].rule.as_str(), t[0].line), ("TS2322", 12));
}

#[test]
fn security_score_is_deterministic() {
    let c = BTreeMap::from([("high".to_string(), 2), ("medium".to_string(), 2)]);
    assert_eq!(security_score(&c), 5.0);
    assert_eq!(
        security_score(&BTreeMap::from([("critical".to_string(), 5)])),
        0.0
    );
}

// ------------------------------------------------------------------ metrics

#[test]
fn measures_python_rust_and_js_functions() {
    let py = "def simple(x):\n    return x\n\ndef branchy(x):\n    if x:\n        for i in x:\n            if i and x:\n                pass\n    elif x is None:\n        pass\n    return 1\n";
    let f = functions("a.py", py);
    assert_eq!(f.len(), 2);
    assert_eq!((f[0].name.as_str(), f[0].complexity), ("simple", 1));
    assert!(f[1].complexity >= 5, "{}", f[1].complexity);
    assert!(f[1].max_nesting >= 3);
    let rs = "pub fn a(x: i32) -> i32 {\n    if x > 0 { 1 } else { match x { 0 => 0, _ => 2 } }\n}\nfn b() {}\n";
    let r = functions("a.rs", rs);
    assert_eq!(r.len(), 2);
    assert!(r[0].complexity >= 3);
    let js = "export async function load(u) {\n  if (!u) return null;\n  return fetch(u).then(r => r.ok ? r.json() : null);\n}\nconst add = (a, b) => {\n  return a + b;\n};\n";
    let j = functions("a.js", js);
    assert_eq!(
        j.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
        vec!["load", "add"]
    );
    // braces inside strings don't confuse the extent
    let tricky = "fn s() {\n    let x = \"}}}\";\n    let y = 1;\n}\nfn t() {}\n";
    assert_eq!(functions("t.rs", tricky)[0].lines, 4);
}

#[test]
fn finds_duplicate_blocks() {
    let block: String = (0..10)
        .map(|i| format!("    total += compute_value(item_{i}, factor);\n"))
        .collect();
    let files = vec![
        ("a.rs".to_string(), format!("fn a() {{\n{block}}}\n")),
        ("b.rs".to_string(), format!("fn b() {{\n{block}}}\n")),
    ];
    let d = duplicates(&files, 8);
    assert!(!d.is_empty());
    assert!(d[0].first.starts_with("a.rs") && d[0].second.starts_with("b.rs"));
}

// ------------------------------------------------------------------ rules

#[test]
fn budgets_and_leak_heuristics() {
    let stats = json!({"cpu_avg_pct": 180.0, "rss_peak_mb": 900.0, "rss_slope_mb_per_min": 40.0, "duration_s": 30.0,
                       "handles_start": 100, "handles_end": 900, "exited_early": false, "gpu_util_avg_pct": 5.0});
    let v = check_budgets(
        &stats,
        &Budgets {
            max_cpu_avg_pct: Some(100.0),
            max_rss_mb: Some(500.0),
            ..Default::default()
        },
    );
    assert_eq!(v.len(), 4, "{v:?}"); // cpu, rss, slope, handles
    let calm = json!({"cpu_avg_pct": 3.0, "rss_peak_mb": 80.0, "rss_slope_mb_per_min": 0.4, "duration_s": 30.0, "handles_start": 300, "handles_end": 310});
    assert!(check_budgets(&calm, &Budgets::default()).is_empty());
    // short runs don't judge slope
    let short = json!({"rss_slope_mb_per_min": 90.0, "duration_s": 5.0, "handles_start": 0, "handles_end": 0});
    assert!(check_budgets(&short, &Budgets::default()).is_empty());
}

#[test]
fn desktop_judging() {
    let rep = json!({"ok": true,
        "interactions": [
            {"element": {"control_type": "Button", "name": "Save"}, "result": "ok", "errors": []},
            {"element": {"control_type": "Button", "name": "Load"}, "result": "error_dialog", "errors": ["Application Error: boom"]},
            {"element": {"control_type": "Button", "name": "Inert"}, "result": "no_effect", "errors": []},
            {"element": {"control_type": "Button", "name": "Tooltip"}, "result": "no_effect", "errors": []},
            {"element": {"control_type": "Button", "name": "Delete"}, "result": "skipped", "errors": []}],
        "persistence": [{"auto_id": "nameBox", "persisted": false, "expected": "a", "actual": ""}],
        "stderr_tail": "info: started\nERROR: cache failed\n"});
    let (s, b, a) = judge_desktop(&rep, &["Tooltip".to_string()]);
    assert_eq!(a, Some(false));
    assert_eq!(b.len(), 3, "{b:?}"); // error dialog, persistence, stderr; inert is unverified
    assert_eq!(s["unverified_effects"], 1);
    assert_eq!(s["skipped"], 1);
    let (_, _, a) = judge_desktop(&json!({"ok": false, "error": "no window"}), &[]);
    assert_eq!(a, None);
    let (_, b, a) = judge_desktop(
        &json!({"ok": false, "error": "exited during startup with code 7", "crashes": [{"after": "startup", "exit_code": 7}]}),
        &[],
    );
    assert_eq!((a, b.len()), (Some(false), 1));
}

// ------------------------------------------------------------------ quality with a fake Kev

struct FakeKev;
impl Decider for FakeKev {
    async fn decide(
        &self,
        state: &Value,
        qs: &BTreeMap<String, Question>,
    ) -> Result<Decision, String> {
        // The spec shows Kev the function, not its metrics (code judges those).
        let hard = state["function"]
            .as_str()
            .unwrap_or("")
            .matches("if ")
            .count()
            > 10;
        let answers = qs
            .keys()
            .map(|k| {
                (
                    k.clone(),
                    Answer {
                        value: if hard { 0.9 } else { 0.1 },
                        choice: None,
                        probabilities: None,
                        confidence: 0.9,
                    },
                )
            })
            .collect();
        Ok(Decision {
            answers,
            truncated: false,
            latency_ms: 1,
            input_tokens: 100,
        })
    }
}

#[tokio::test]
async fn quality_flags_complex_functions_and_scores_maintainability() {
    let mut cfg = Config::from_env();
    cfg.max_hotspots = 5;
    let simple = (
        "ok.py".to_string(),
        "def f(x):\n    return x + 1\n".to_string(),
    );
    let branches: String = (0..25)
        .map(|i| format!("    if x == {i}:\n        y += {i}\n"))
        .collect();
    let complex = (
        "bad.py".to_string(),
        format!("def g(x):\n    y = 0\n{branches}    return y\n"),
    );
    let opts = QualityOptions {
        max_complexity: 20,
        max_function_lines: 150,
        run_linters: false,
        use_kev: true,
    };
    let r = phase3::quality(
        &FakeKev,
        &cfg,
        Path::new("."),
        &[simple.clone(), complex],
        opts,
    )
    .await;
    assert_eq!(r.approval, Some(false));
    assert!(
        r.reasons
            .iter()
            .any(|x| x.contains("g in bad.py") && x.contains("complexity")),
        "{:?}",
        r.reasons
    );
    assert_eq!(r.hotspots[0].metric.name, "g");
    assert_eq!(r.hotspots[0].p_hard_to_maintain, Some(0.9));
    assert!(r.maintainability.unwrap() < 5.0);
    let opts = QualityOptions {
        max_complexity: 20,
        max_function_lines: 150,
        run_linters: false,
        use_kev: true,
    };
    let clean = phase3::quality(&FakeKev, &cfg, Path::new("."), &[simple], opts).await;
    assert_eq!(clean.approval, Some(true), "{:?}", clean.reasons);
    assert_eq!(clean.risk_level, "low");
}

// ------------------------------------------------------------------ live

fn live() -> bool {
    std::env::var("KRITIC_LIVE").as_deref() == Ok("1")
}

fn vuln_project(dir: &Path) {
    // assembled at runtime so this repo never contains a secret-looking literal
    let key = ["AKIA", "QYLPMN5HHHFPZAM2"].concat();
    std::fs::write(
        dir.join("app.py"),
        format!(
            "import sqlite3\nimport subprocess\n\nAWS_KEY = \"{key}\"\n\n\ndef run(cmd):\n    return subprocess.call(cmd, shell=True)\n\n\ndef calc(expr):\n    return eval(expr)\n\n\ndef find(db, name):\n    return db.execute(\"SELECT * FROM users WHERE name = '\" + name + \"'\").fetchone()\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("requirements.txt"), "requests==2.19.0\n").unwrap();
}

#[tokio::test]
async fn live_security_vulnerable_vs_clean() {
    if !live() {
        return;
    }
    let cfg = Config::from_env();
    let kev = KevClient::new(std::sync::Arc::new(Config::from_env()));
    let bad = tempfile::tempdir().unwrap();
    vuln_project(bad.path());
    let r = phase3::security(&kev, &cfg, bad.path(), true, "p/python").await;
    eprintln!(
        "vulnerable: approval={:?} score={:?} counts={:?} {}ms",
        r.approval, r.security_score, r.counts, r.duration_ms
    );
    for t in &r.tools {
        eprintln!(
            "  tool {} {} {} findings {}",
            t.tool, t.status, t.detail, t.findings
        );
    }
    for f in r.findings.iter().take(10) {
        eprintln!(
            "  [{}] {} {} {}:{} p_true={:?}",
            f.finding.severity,
            f.finding.tool,
            f.finding.rule,
            f.finding.file,
            f.finding.line,
            f.p_true_positive
        );
    }
    assert_eq!(r.approval, Some(false));
    let tools: Vec<&str> = r.findings.iter().map(|f| f.finding.tool.as_str()).collect();
    for t in ["gitleaks", "bandit", "osv-scanner"] {
        assert!(tools.contains(&t), "no {t} finding");
    }
    assert!(
        r.findings
            .iter()
            .all(|f| !f.finding.message.contains("QYLPMN5")),
        "secret leaked into output"
    );

    let clean = tempfile::tempdir().unwrap();
    std::fs::write(
        clean.path().join("util.py"),
        "def add(a: int, b: int) -> int:\n    return a + b\n",
    )
    .unwrap();
    let c = phase3::security(&kev, &cfg, clean.path(), true, "p/python").await;
    eprintln!(
        "clean: approval={:?} score={:?} counts={:?} reasons={:?}",
        c.approval, c.security_score, c.counts, c.reasons
    );
    assert_eq!(c.approval, Some(true));
}

#[tokio::test]
async fn live_quality_on_this_crate() {
    if !live() {
        return;
    }
    let cfg = Config::from_env();
    let kev = KevClient::new(std::sync::Arc::new(Config::from_env()));
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let paths = kritic::tools::source_files(&base.join("src"), 80);
    let lf = kritic::tools::load_files(&paths, base.to_str()).await;
    let files: Vec<(String, String)> = lf
        .files
        .into_iter()
        .map(|(f, t)| {
            (
                Path::new(&f)
                    .strip_prefix(base)
                    .unwrap()
                    .display()
                    .to_string(),
                t,
            )
        })
        .collect();
    let r = phase3::quality(
        &kev,
        &cfg,
        base,
        &files,
        QualityOptions {
            max_complexity: 20,
            max_function_lines: 150,
            run_linters: true,
            use_kev: true,
        },
    )
    .await;
    eprintln!(
        "crate: approval={:?} score={:?} risk={} maint={:?} fns={} maxcx={} maxlen={} dups={} {}ms",
        r.approval,
        r.quality_score,
        r.risk_level,
        r.maintainability,
        r.functions_analyzed,
        r.max_complexity,
        r.max_function_lines,
        r.duplicates.len(),
        r.duration_ms
    );
    for t in &r.tools {
        eprintln!(
            "  tool {} {} {} findings {}",
            t.tool, t.status, t.detail, t.findings
        );
    }
    for h in &r.hotspots {
        eprintln!(
            "  {}:{} {} lines={} cx={} p_hard={:?}",
            h.metric.file,
            h.metric.start_line,
            h.metric.name,
            h.metric.lines,
            h.metric.complexity,
            h.p_hard_to_maintain
        );
    }
    for x in &r.reasons {
        eprintln!("  reason: {x}");
    }
    assert!(r.kev_ok == Some(true) && r.functions_analyzed > 50);
    assert!(
        r.tools
            .iter()
            .any(|t| t.tool == "clippy" && t.status == "ran")
    );
}

#[tokio::test]
async fn live_desktop_fixture_via_rust() {
    if !live() || !cfg!(windows) {
        return;
    }
    let cfg = Config::from_env();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("harness/py/tests/fixture_app.ps1");
    let state = tempfile::tempdir().unwrap();
    let args = |mode: &str| -> Vec<String> {
        [
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &fixture.display().to_string(),
            "-Mode",
            mode,
            "-StateFile",
            &state.path().join("s.json").display().to_string(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
    let persist = vec![json!({"auto_id": "nameBox"})];
    let good = phase3::desktop_test(
        &cfg,
        "powershell.exe",
        &args("good"),
        None,
        Some("Kritic Fixture"),
        &persist,
        60,
        false,
        false,
        5.0,
        &[],
        &[],
        true,
        &json!({}),
    )
    .await;
    eprintln!(
        "good: approval={:?} summary={:?} breakages={:?} err={:?} {}ms",
        good.approval, good.summary, good.breakages, good.error, good.duration_ms
    );
    assert_eq!(good.approval, Some(true));
    let bad = phase3::desktop_test(
        &cfg,
        "powershell.exe",
        &args("broken"),
        None,
        Some("Kritic Fixture"),
        &persist,
        60,
        false,
        false,
        5.0,
        &[],
        &[],
        true,
        &json!({}),
    )
    .await;
    eprintln!(
        "broken: approval={:?} summary={:?}",
        bad.approval, bad.summary
    );
    for b in &bad.breakages {
        eprintln!("  - {b}");
    }
    assert_eq!(bad.approval, Some(false));
    let all = bad.breakages.join("\n");
    for needle in [
        "Object reference",
        "Does nothing",
        "crash",
        "did not persist",
        "widget cache failed",
    ] {
        assert!(all.contains(needle), "missing {needle}");
    }
}

#[tokio::test]
async fn live_resource_leaky_vs_idle() {
    if !live() {
        return;
    }
    let cfg = Config::from_env();
    let kev = KevClient::new(std::sync::Arc::new(Config::from_env()));
    let leaky = json!([
        "python",
        "-c",
        "import time\nx=[]\nwhile True:\n    x.append(bytearray(400000)); time.sleep(0.02)"
    ]);
    let r = phase3::resource(
        &kev,
        &cfg,
        Some(leaky),
        None,
        false,
        None,
        20.0,
        "a CLI tool idling",
        Budgets::default(),
    )
    .await;
    eprintln!(
        "leaky: approval={:?} p_fits={:?} violations={:?} stats={}",
        r.approval, r.p_reasonable, r.budget_violations, r.stats
    );
    assert_eq!(r.approval, Some(false));
    assert!(
        r.budget_violations
            .iter()
            .any(|v| v.contains("memory growing"))
    );
    let idle = json!(["python", "-c", "import time\ntime.sleep(60)"]);
    let r = phase3::resource(
        &kev,
        &cfg,
        Some(idle),
        None,
        false,
        None,
        20.0,
        "a CLI tool idling",
        Budgets::default(),
    )
    .await;
    eprintln!(
        "idle: approval={:?} p_fits={:?} violations={:?}",
        r.approval, r.p_reasonable, r.budget_violations
    );
    assert!(r.budget_violations.is_empty());
}

// ------------------------------------------------------------------ fixes from self-review

#[test]
fn vendored_paths_are_recognized() {
    for p in [
        "harness/py/.venv/Lib/site-packages/x.py",
        r"a\node_modules\b.js",
        "target/debug/x.rs",
        "web/dist/app.js",
    ] {
        assert!(scan::is_vendored(p), "{p}");
    }
    for p in ["src/venv_utils.py", "src/targeting.rs", "app.py"] {
        assert!(!scan::is_vendored(p), "{p}");
    }
    assert!(scan::bandit_excludes().contains("*/.venv/*"));
}

#[test]
fn gitleaks_config_extends_defaults_and_allowlists_vendored() {
    let d = tempfile::tempdir().unwrap();
    let p = scan::gitleaks_config(d.path()).unwrap();
    let body = std::fs::read_to_string(p).unwrap();
    assert!(body.contains("useDefault = true"));
    let t: toml::Table = body.parse().expect("valid TOML");
    let pat = t["allowlist"]["paths"][0].as_str().unwrap();
    let re = regex::Regex::new(pat).unwrap();
    assert!(re.is_match("harness/py/.venv/Lib/x.py") && re.is_match(r"a\site-packages\y.py"));
    assert!(!re.is_match("src/venv_utils.py"));
}

struct TruncatingKev;
impl Decider for TruncatingKev {
    async fn decide(
        &self,
        state: &Value,
        qs: &BTreeMap<String, Question>,
    ) -> Result<Decision, String> {
        // pretend anything over 600 chars of diff was cut off
        let truncated = state["diff"].as_str().is_some_and(|d| d.len() > 600);
        let answers = qs
            .iter()
            .map(|(k, q)| {
                let a = if q.kind == "choice" {
                    Answer {
                        value: 0.9,
                        choice: Some("none".into()),
                        probabilities: Some(BTreeMap::from([("none".into(), 0.9)])),
                        confidence: 0.9,
                    }
                } else {
                    Answer {
                        value: if k == "complete" { 0.9 } else { 0.05 },
                        choice: None,
                        probabilities: None,
                        confidence: 0.9,
                    }
                };
                (k.clone(), a)
            })
            .collect();
        Ok(Decision {
            answers,
            truncated,
            latency_ms: 1,
            input_tokens: 100,
        })
    }
}

#[tokio::test]
async fn diff_mode_resplits_truncated_hunks() {
    let body: String = (0..40)
        .map(|i| format!("+let value_{i} = compute(input_{i});\n"))
        .collect();
    let diff = format!("diff --git a/x.rs b/x.rs\n@@ -0,0 +1,40 @@\n{body}");
    let cfg = Config::from_env();
    let r = kritic::critic::critic_diff(&TruncatingKev, &cfg, "t", &diff, 0).await;
    assert_eq!(r.truncated_chunks, 0, "halves should fit");
    assert!(r.chunks_scored > 1);
    assert_eq!(r.approval, Some(true), "{:?}", r.reasons);
}

#[tokio::test]
async fn gpu_file_lock_is_exclusive_and_released_on_drop() {
    let d = tempfile::tempdir().unwrap();
    let mut cfg = Config::from_env();
    cfg.runs_dir = d.path().to_path_buf();
    cfg.gpu_wait_s = 1;
    let a = kritic::gpu::lock_across_processes(&cfg, "a").await.unwrap();
    // a second handle (as another process would open) cannot take it while `a` holds it
    let busy = kritic::gpu::lock_across_processes(&cfg, "b").await;
    assert!(busy.is_err(), "second holder must wait");
    drop(a);
    assert!(kritic::gpu::lock_across_processes(&cfg, "c").await.is_ok());
}

// ------------------------------------------------------------------ Unity

#[test]
fn parses_unity_nunit_results() {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<test-run id="2" testcasecount="3" result="Failed(Child)" total="3" passed="1" failed="1" inconclusive="0" skipped="1" asserts="0">
  <test-suite type="Assembly" name="Tests.dll">
    <test-case id="1001" name="Adds" fullname="Game.Tests.MathTests.Adds" result="Passed" duration="0.01" />
    <test-case id="1002" name="Damage" fullname="Game.Tests.CombatTests.Damage" result="Failed" duration="0.02">
      <failure><message><![CDATA[  Expected: 10
  But was:  7
]]></message><stack-trace><![CDATA[at CombatTests.Damage ()]]></stack-trace></failure>
    </test-case>
    <test-case id="1003" name="Later" fullname="Game.Tests.X.Later" result="Skipped" />
  </test-suite>
</test-run>"#;
    let (total, passed, failed, skipped, failures) = kritic::unity::parse_nunit(xml).unwrap();
    assert_eq!((total, passed, failed, skipped), (3, 1, 1, 1));
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].name, "Game.Tests.CombatTests.Damage");
    assert!(failures[0].message.contains("Expected: 10"));
    assert!(kritic::unity::parse_nunit("<nope/>").is_err());
}

#[test]
fn unity_soak_judging() {
    let rep = json!({"ok": true, "interactions": [], "persistence": [], "stderr_tail": "",
        "crashes": [{"after": "soak 3.2s", "exit_code": 5}], "hangs": [],
        "unity": {"log": {"counts": {"exception": 2}, "samples": {"exception": ["NullReferenceException: x"]}},
                  "steps": [{"do": "key", "result": "ok"}],
                  "frames": {"ok": true, "avg_fps": 41.0, "low_1pct_fps": 20.0, "p99_ms": 50.0, "stutter_pct": 4.0}}});
    let (_, b, a) = phase3::judge_desktop_with(&rep, &[], Some(55.0), Some(2.0));
    assert_eq!(a, Some(false));
    let all = b.join("\n");
    for needle in [
        "crashed during soak 3.2s",
        "2 exception",
        "41.0 fps < 55",
        "4.0% of frames stutter",
    ] {
        assert!(all.contains(needle), "missing {needle}: {all}");
    }
    let clean = json!({"ok": true, "interactions": [], "crashes": [], "hangs": [], "unity": {"log": {"counts": {}}, "steps": [], "frames": {"ok": false, "error": "PresentMon needs admin"}}});
    assert_eq!(
        phase3::judge_desktop_with(&clean, &[], None, None).2,
        Some(true)
    );
    let (_, b, _) = phase3::judge_desktop_with(&clean, &[], Some(60.0), None);
    assert!(b[0].contains("no frame timing") && b[0].contains("PresentMon needs admin"));
}

#[tokio::test]
async fn unity_tests_reports_missing_editor_honestly() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("ProjectSettings")).unwrap();
    std::fs::write(
        d.path().join("ProjectSettings/ProjectVersion.txt"),
        "m_EditorVersion: 1999.1.1f1\n",
    )
    .unwrap();
    let mut cfg = Config::from_env();
    cfg.unity_editor = None;
    let r = kritic::unity::run_tests(&cfg, d.path(), "EditMode", None, 60).await;
    assert!(r.error.unwrap().contains("1999.1.1f1 not installed"));
    assert_eq!(r.approval, None);
}

#[test]
fn desktop_unobserved_effect_is_unknown_not_a_broken_handler() {
    let rep = json!({"ok":true,"interactions":[{"element":{"name":"Paint"},"result":"no_effect"}]});
    let (s, b, a) = judge_desktop(&rep, &[]);
    assert_eq!(a, None);
    assert!(b.is_empty());
    assert_eq!(s["unverified_effects"], 1);
}

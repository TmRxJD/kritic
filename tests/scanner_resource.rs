use kritic::{native::resource, scan};
use std::path::Path;

#[test]
fn explicit_scanner_extension_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("tsc.cmd");
    std::fs::write(&script, "@exit /b 0").unwrap();
    assert_eq!(scan::tool_path(dir.path(), "tsc.cmd"), Some(script));
}

#[test]
fn scanner_exit_and_completion_are_not_findings() {
    assert!(scan::completion_error("clippy", "", 101, 0).is_some());
    assert!(scan::completion_error("tsc", "error TS18003: No inputs were found", 2, 0).is_some());
    assert!(scan::completion_error("clippy", "{}", 0, 0).is_some());
    assert!(
        scan::completion_error(
            "clippy",
            "{\"reason\":\"build-finished\",\"success\":true}",
            0,
            0
        )
        .is_none()
    );
    assert!(scan::completion_error("eslint", "[]", 1, 0).is_some());
    assert!(scan::completion_error("eslint", "[]", 2, 1).is_some());
    assert!(
        scan::completion_error(
            "bandit",
            "{\"results\":[],\"metrics\":{\"_totals\":{\"loc\":1}}}",
            1,
            1
        )
        .is_none()
    );
}

#[test]
fn malformed_and_partial_scanner_reports_are_not_clean() {
    assert!(scan::parse_osv(Path::new("."), "{}").is_err());
    assert!(scan::parse_osv(Path::new("."), r#"{"results":[{"packages":{}}]}"#).is_err());
    assert!(scan::parse_eslint(Path::new("."), r#"[{"filePath":"a.js"}]"#).is_err());
    let report = r#"{"results":[{"check_id":"x","path":"a.rs","start":{"line":1},"extra":{"severity":"ERROR","message":"bad"}}],"errors":[{"message":"timeout"}]}"#;
    assert_eq!(
        scan::parse_semgrep(Path::new("."), report).unwrap().len(),
        1
    );
    assert!(scan::completion_error("semgrep", report, 0, 1).is_some());
    assert!(scan::completion_error("cargo-audit", "{}", 0, 0).is_some());
    assert!(scan::completion_error("semgrep", r#"{"results":[{}]}"#, 0, 0).is_some());
    assert!(
        scan::completion_error("eslint", r#"[{"filePath":"a.js","messages":[{}]}]"#, 0, 0)
            .is_some()
    );
}

#[tokio::test]
async fn missing_applicable_scanner_is_failed() {
    let mut runner = scan::Runner::new(Path::new("."), Path::new("."), 1);
    runner.run("semgrep", None, &[], |_| Ok(Vec::new())).await;
    assert_eq!(runner.runs[0].status, "failed");
    runner.skip("bandit", "no Python code");
    assert_eq!(runner.runs[1].status, "skipped");
}

#[tokio::test]
async fn failed_scanner_preserves_real_partial_findings() {
    let report = r#"{"results":[{"check_id":"x","path":"a.rs","start":{"line":1},"extra":{"severity":"ERROR","message":"bad"}}]}"#;
    let dir = tempfile::tempdir().unwrap();
    let report_path = dir.path().join("report.json");
    std::fs::write(&report_path, report).unwrap();
    std::fs::write(
        dir.path().join("fixture.cmd"),
        "@type report.json\r\n@exit /b 7\r\n",
    )
    .unwrap();
    let (exe, args) = if cfg!(windows) {
        (
            std::path::PathBuf::from(
                std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()),
            ),
            vec![
                "/D".to_string(),
                "/C".to_string(),
                "fixture.cmd".to_string(),
            ],
        )
    } else {
        (
            std::path::PathBuf::from("/bin/sh"),
            vec![
                "-c".to_string(),
                format!("printf '%s\\n' '{report}'; exit 7"),
            ],
        )
    };
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let mut runner = scan::Runner::new(dir.path(), dir.path(), 5);
    runner
        .run("semgrep", Some(exe), &args, |out| {
            scan::parse_semgrep(Path::new("."), out)
        })
        .await;
    assert_eq!(runner.runs[0].status, "failed");
    assert!(runner.runs[0].detail.contains("exited with 7"));
    assert_eq!(runner.findings.len(), 1);
    assert_eq!(runner.runs[0].findings, 1);
}

#[tokio::test]
async fn nonexistent_pid_never_becomes_zero_usage_success() {
    let result = resource::resource(None, Some(u32::MAX), true, None, 0.2, 100, 0.0).await;
    assert_eq!(result["ok"], false);
    assert!(result["error"].as_str().unwrap().contains("identity"));
}

#[tokio::test]
async fn live_attachment_collects_actual_samples_without_gpu_probe() {
    let result =
        resource::resource(None, Some(std::process::id()), true, None, 0.2, 100, 0.0).await;
    assert_eq!(result["ok"], true, "{result}");
    assert!(result["stats"]["samples"].as_u64().unwrap() > 0);
    assert!(result["stats"]["rss_peak_mb"].as_f64().unwrap() > 0.0);
    assert!(result["stats"]["gpu_util_avg_pct"].is_null());
}

#[tokio::test]
async fn attached_process_disappearance_is_incomplete() {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "resource_sleep_fixture", "--ignored"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let sampling = tokio::spawn(resource::resource(
        None,
        Some(pid),
        true,
        None,
        0.8,
        100,
        0.5,
    ));
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    child.kill().unwrap();
    child.wait().unwrap();
    let result = sampling.await.unwrap();
    assert_eq!(result["ok"], false, "{result}");
    assert!(
        result.get("stats").is_none(),
        "missing process must not report zero usage"
    );
}

#[test]
#[ignore = "child-process fixture, launched only by the disappearance regression"]
fn resource_sleep_fixture() {
    std::thread::sleep(std::time::Duration::from_secs(10));
}

#[test]
fn zero_scanner_coverage_is_unverified() {
    assert!(
        scan::completion_error(
            "semgrep",
            r#"{"results":[],"errors":[],"paths":{"scanned":[]}}"#,
            0,
            0
        )
        .is_some()
    );
    assert!(
        scan::completion_error(
            "bandit",
            r#"{"results":[],"metrics":{"_totals":{"loc":0}}}"#,
            0,
            0
        )
        .is_some()
    );
    assert!(
        scan::completion_error(
            "semgrep",
            r#"{"results":[],"errors":[],"paths":{"scanned":["a.rs"]}}"#,
            0,
            0
        )
        .is_none()
    );
}

#[test]
fn scanner_errors_retain_actionable_bounded_diagnostics() {
    let report = serde_json::json!({"results":[], "errors":[{
        "path":"scripts/install.sh", "message":"unexpected shell case syntax"
    }]});
    let error = scan::completion_error("semgrep", &report.to_string(), 0, 0).unwrap();
    assert!(error.contains("scripts/install.sh"));
    assert!(error.contains("unexpected shell case syntax"));
    let report = serde_json::json!({"results":[], "errors":[{"message":"x".repeat(10000)}]});
    assert!(
        scan::completion_error("semgrep", &report.to_string(), 0, 0)
            .unwrap()
            .len()
            < 400
    );
}

#[test]
fn maintenance_notices_are_not_vulnerabilities_but_mixed_security_stays_high() {
    use serde_json::json;
    let info = json!({"id":"RUSTSEC-2024-0436", "affected":[{"database_specific":{"informational":"unmaintained"}}]});
    let mut report = json!({"results":[{"source":{"path":"Cargo.lock"},"packages":[{
        "package":{"name":"paste","version":"1.0.15"},"groups":[],"vulnerabilities":[info]
    }]}]});
    let findings = scan::parse_osv(Path::new("."), &report.to_string()).unwrap();
    assert_eq!(findings[0].severity, "info");
    assert_eq!(findings[0].rule, "informational-dependency");
    report["results"][0]["packages"][0]["vulnerabilities"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"CVE-TEST","affected":[{}]}));
    report["results"][0]["packages"][0]["groups"] = json!([{"max_severity":"9.8"}]);
    let mixed = scan::parse_osv(Path::new("."), &report.to_string()).unwrap();
    assert_eq!(mixed[0].severity, "critical");
    assert_eq!(mixed[0].rule, "vulnerable-dependency");
}

use clap::Parser;
use kritic::{
    cli::Cli,
    config::Config,
    history_api::{Request, execute},
};
use serde_json::json;

#[test]
fn history_request_schema_has_the_object_root_required_by_mcp() {
    let schema = schemars::schema_for!(Request);
    assert_eq!(schema.as_value()["type"], "object");
    assert_eq!(schema.as_value()["oneOf"].as_array().unwrap().len(), 4);
    let cfg = Config::load();
    // Constructing the real router validates every registered input schema.
    let _server = kritic::server::Kritic::new(cfg);
}

#[test]
fn disabled_history_never_creates_storage_or_reads_report() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = Config::load();
    cfg.runs_dir = temp.path().join("not-created");
    cfg.capabilities.insert("history".into(), false);
    let request = || Request::Record {
        base_dir: "missing-repository".into(),
        report: "missing-report".into(),
        inputs: vec!["missing.rs".into()],
        context: "tool=fixture;model=off".into(),
    };
    assert!(execute(&cfg, request()).unwrap_err().contains("disabled"));
    assert!(!cfg.runs_dir.exists());
    cfg.capabilities.insert("history".into(), true);
    cfg.disabled_tools.push("review_history".into());
    assert!(execute(&cfg, request()).unwrap_err().contains("disabled"));
    assert!(!cfg.runs_dir.exists());
}

#[test]
fn tagged_requests_and_cli_reject_missing_or_ambiguous_record_inputs() {
    for value in [
        json!({"action":"record","base_dir":".","report":"report.json","context":"model=off"}),
        json!({"action":"list","base_dir":".","approval":true}),
        json!({"action":"load_cached_pass","base_dir":"."}),
    ] {
        assert!(serde_json::from_value::<Request>(value).is_err());
    }
    assert!(
        Cli::try_parse_from([
            "kritic",
            "history",
            "record",
            "--report",
            "report.json",
            "--context",
            "model=off"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "kritic",
            "history",
            "--base-dir",
            ".",
            "record",
            "--report",
            "report.json",
            "--inputs",
            "src/a.rs",
            "--context",
            "tool=fixture;model=off"
        ])
        .is_ok()
    );
    assert!(Cli::try_parse_from(["kritic", "history", "export-jcode", "--base-dir", "."]).is_ok());
}

#[test]
fn history_api_records_compares_and_exports_without_reusing_approval() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let report = temp.path().join("report.json");
    let mut cfg = Config::load();
    cfg.runs_dir = temp.path().join("runs");
    cfg.capabilities.insert("history".into(), true);
    cfg.disabled_tools.retain(|name| name != "review_history");
    let git = kritic::proc::hidden_command("git")
        .arg("-C")
        .arg(&repo)
        .arg("init")
        .output()
        .unwrap();
    assert!(git.status.success());
    std::fs::write(repo.join("a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(
        &report,
        r#"{"tool":"artifact_validate","approval":true,"marker":"PRIVATE_REPORT"}"#,
    )
    .unwrap();
    let base_dir = repo.to_string_lossy().into_owned();
    let record = || Request::Record {
        base_dir: base_dir.clone(),
        report: report.to_string_lossy().into_owned(),
        inputs: vec!["a.rs".into()],
        context: "tool=artifact_validate;commands=fixture-v1;model=off".into(),
    };
    let first = execute(&cfg, record()).unwrap();
    assert_eq!(first["historical_unverified"], true);
    assert!(first.get("approval").is_none() && first.get("report").is_none());
    let id = first["id"].as_str().unwrap().to_owned();
    assert_eq!(execute(&cfg, record()).unwrap()["id"], id);
    std::fs::write(repo.join("a.rs"), "fn main() { panic!(); }\n").unwrap();
    let second = execute(&cfg, record()).unwrap();
    assert_ne!(
        first["id"], second["id"],
        "a stale passing report must not hide changed inputs"
    );
    let comparison = execute(
        &cfg,
        Request::Compare {
            base_dir: base_dir.clone(),
            left: id,
            right: second["id"].as_str().unwrap().into(),
        },
    )
    .unwrap();
    assert_eq!(comparison["historical_unverified"], true);
    assert_eq!(comparison["changed_inputs"], json!(["a.rs"]));
    assert!(comparison.get("approval").is_none());
    let listed = execute(
        &cfg,
        Request::List {
            base_dir: base_dir.clone(),
            offset: 0,
        },
    )
    .unwrap();
    assert_eq!(listed["total"], 2);
    let export = execute(&cfg, Request::ExportJcode { base_dir }).unwrap();
    assert_eq!(export["live_jcode_modified"], false);
    let bytes = std::fs::read_to_string(export["export_path"].as_str().unwrap()).unwrap();
    assert!(!bytes.contains("PRIVATE_REPORT") && !bytes.contains("a.rs"));
    let value: serde_json::Value = serde_json::from_str(&bytes).unwrap();
    assert_eq!(value["entries"].as_array().unwrap().len(), 2);
}

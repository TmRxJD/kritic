use kritic::config::{Config, TOOL_NAMES};
use kritic::contract::critique;
use kritic::setup_guidance::{ProjectSetup, inspect};

fn cfg() -> Config {
    let mut cfg = Config::load();
    cfg.backend = "off".into();
    cfg.vision_backend = "off".into();
    cfg.disabled_tools.clear();
    for enabled in cfg.capabilities.values_mut() {
        *enabled = true;
    }
    cfg
}

fn tool<'a>(report: &'a ProjectSetup, name: &str) -> &'a kritic::setup_guidance::ToolSetup {
    report.tools.iter().find(|tool| tool.tool == name).unwrap()
}

#[test]
fn every_discoverable_tool_has_guidance_without_executing_or_creating_anything() {
    let root = tempfile::tempdir().unwrap();
    let report = inspect(&cfg(), root.path().to_str().unwrap()).unwrap();
    assert_eq!(report.tools.len(), TOOL_NAMES.len());
    for name in TOOL_NAMES {
        let item = tool(&report, name);
        assert!(!item.next_step.trim().is_empty(), "{name}");
        assert!(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(&item.guide)
                .is_file(),
            "{}: {}",
            name,
            item.guide
        );
        assert!(
            include_str!("../examples/claude/skills/kritic/references/setup.md")
                .contains(&format!("`{name}`")),
            "Missing installed setup reference for {name}"
        );
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert_eq!(tool(&report, "contract_critic").config_state, "missing");
    assert_eq!(tool(&report, "ux_critic").config_state, "missing");
    assert_eq!(tool(&report, "framing_critic").config_state, "missing");
}

#[test]
fn project_preflight_is_opt_in_in_existing_status_and_doctor_interfaces() {
    use clap::Parser;
    let old: kritic::server::StatusParams =
        serde_json::from_value(serde_json::json!({"start":false})).unwrap();
    assert!(old.base_dir.is_none());
    let scoped: kritic::server::StatusParams =
        serde_json::from_value(serde_json::json!({"base_dir":"."})).unwrap();
    assert_eq!(scoped.base_dir.as_deref(), Some("."));
    assert!(!scoped.start);
    assert!(
        kritic::cli::Cli::try_parse_from(["kritic", "doctor", "--base-dir", ".", "--json"]).is_ok()
    );
    assert!(kritic::cli::Cli::try_parse_from(["kritic", "doctor", "--json"]).is_ok());
}

#[test]
fn an_invalid_manifest_is_present_unvalidated_never_reported_as_ready() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".kritic")).unwrap();
    std::fs::write(
        root.path().join(".kritic/contract.toml"),
        "not valid TOML [",
    )
    .unwrap();
    let report = inspect(&cfg(), root.path().to_str().unwrap()).unwrap();
    assert_eq!(
        tool(&report, "contract_critic").config_state,
        "present_unvalidated"
    );
    let json = serde_json::to_value(report).unwrap();
    assert!(json.get("approval").is_none());
    assert!(json.get("ready").is_none());
}

#[test]
fn disabled_and_inapplicable_critics_do_not_request_blind_enablement() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = cfg();
    cfg.capabilities.insert("contracts".into(), false);
    cfg.disabled_tools.push("artifact_validate".into());
    let report = inspect(&cfg, root.path().to_str().unwrap()).unwrap();
    assert_eq!(tool(&report, "contract_critic").config_state, "disabled");
    assert!(
        tool(&report, "artifact_validate")
            .next_step
            .contains("tools.disabled")
    );
    assert!(
        !tool(&report, "artifact_validate")
            .next_step
            .contains("select the authorized kev.backend")
    );
    assert!(
        tool(&report, "unity_tests")
            .next_step
            .contains("skip this critic")
    );
}

#[test]
fn wrong_path_types_and_invalid_roots_are_not_treated_as_configured() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".kritic/contract.toml")).unwrap();
    let report = inspect(&cfg(), root.path().to_str().unwrap()).unwrap();
    assert_eq!(
        tool(&report, "contract_critic").config_state,
        "wrong_path_type"
    );
    assert!(inspect(&cfg(), root.path().join("missing").to_str().unwrap()).is_err());
    std::fs::write(root.path().join("file"), "a file").unwrap();
    assert!(inspect(&cfg(), root.path().join("file").to_str().unwrap()).is_err());
}

#[tokio::test]
async fn missing_contract_explains_scope_and_recovery_then_clears_after_real_configuration() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = cfg();
    cfg.runs_dir = root.path().join("reports");
    let report = critique(&cfg, root.path(), None, "Project validation", false).await;
    assert_eq!(report.approval, None);
    assert!(report.requirements.is_empty());
    assert_eq!(report.contract_path, ".kritic/contract.toml");
    assert_eq!(
        std::path::Path::new(&report.base_dir),
        root.path().canonicalize().unwrap()
    );
    let setup = report.setup.unwrap();
    assert_eq!(setup.code, "contract_setup_required");
    assert!(
        setup
            .steps
            .iter()
            .any(|step| step.contains("actual CI/hook bindings"))
    );
    assert!(!root.path().join(".kritic/contract.toml").exists());

    std::fs::create_dir(root.path().join(".kritic")).unwrap();
    std::fs::write(
        root.path().join(".kritic/contract.toml"),
        r#"
version=1
title="Actual project validation"
[[requirements]]
id="tests"
description="Run the project's acceptance command"
[[requirements.checks]]
kind="command"
command="echo checked > must-not-run.txt"
"#,
    )
    .unwrap();
    let report = critique(&cfg, root.path(), None, "Project validation", false).await;
    assert!(report.setup.is_none());
    assert_eq!(report.requirements.len(), 1);
    assert_eq!(
        report.approval, None,
        "Unrun commands cannot become approval"
    );
    assert!(!root.path().join("must-not-run.txt").exists());
}

#[tokio::test]
async fn malformed_contract_has_repair_guidance_and_preserves_the_original_error() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = cfg();
    cfg.runs_dir = root.path().join("reports");
    std::fs::create_dir(root.path().join(".kritic")).unwrap();
    std::fs::write(
        root.path().join(".kritic/contract.toml"),
        "version = [broken",
    )
    .unwrap();
    let report = critique(&cfg, root.path(), None, "", false).await;
    assert!(report.setup.is_some());
    assert!(!report.reasons.is_empty());
    assert_eq!(report.approval, None);
    assert_eq!(
        std::fs::read_to_string(root.path().join(".kritic/contract.toml")).unwrap(),
        "version = [broken"
    );
}

#[tokio::test]
async fn doctor_does_not_recommend_autostart_or_owned_assets_for_an_external_reviewer() {
    use kritic::cli::doctor::{Level, checks};
    let root = tempfile::tempdir().unwrap();
    let mut cfg = cfg();
    cfg.backend = "local".into();
    cfg.config_errors.clear();
    cfg.kev_externally_managed = true;
    cfg.kev_gguf = root.path().join("not-owned.gguf");
    cfg.laya_exe = root.path().join("not-owned.exe");
    cfg.laya_url = "http://127.0.0.1:0".into();
    let result = checks(std::sync::Arc::new(cfg), false).await;
    let local: Vec<_> = result.iter().filter(|check| check.area == "kev").collect();
    assert!(!local.iter().any(|check| check.name == "weights"));
    let health = local.iter().find(|check| check.name == "laya").unwrap();
    assert_eq!(health.level, Level::Fail);
    assert!(health.detail.contains("will not start"));
    assert!(!health.detail.contains("starts it on first use"));
    assert!(health.fix.as_ref().unwrap().contains("service owner"));
}

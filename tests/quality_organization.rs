use kritic::config::Config;
use kritic::kev::{Decider, Decision, Question};
use kritic::phase3::{QualityOptions, quality};
use serde_json::Value;
use std::collections::BTreeMap;

struct NoModel;
impl Decider for NoModel {
    async fn decide(&self, _: &Value, _: &BTreeMap<String, Question>) -> Result<Decision, String> {
        panic!("organization checks must not require a model")
    }
}
fn options() -> QualityOptions {
    QualityOptions {
        max_complexity: 20,
        max_function_lines: 150,
        run_linters: false,
        use_kev: false,
    }
}

#[tokio::test]
async fn server_quality_normalizes_loaded_inventory_paths_before_organization_review() {
    use kritic::server::{Kritic, QualityParams};
    use rmcp::handler::server::wrapper::Parameters;

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let source = dir.path().join("src/data.ts");
    std::fs::write(
        &source,
        "// Public data contract.\nexport type Item = { id: string };\n",
    )
    .unwrap();
    let mut config = Config::from_env();
    config.runs_dir = dir.path().join("reports");
    let server = Kritic::new(config);
    for (base, path) in [
        (dir.path().to_path_buf(), "src/data.ts".to_string()),
        (dir.path().to_path_buf(), source.display().to_string()),
        (dir.path().join("."), "src/./data.ts".to_string()),
    ] {
        let report = server
            .code_quality_critic(Parameters(QualityParams {
                base_dir: base.display().to_string(),
                paths: vec![path],
                max_complexity: 20,
                max_function_lines: 150,
                run_linters: false,
                use_kev: false,
            }))
            .await
            .unwrap()
            .0;
        let organization = report.organization.unwrap();
        assert!(organization.approval, "{:?}", organization.violations);
        assert_eq!(organization.files.len(), 1);
        assert_eq!(organization.files[0].path, "src/data.ts");
        assert_eq!(report.approval, Some(true));
    }
}

#[tokio::test]
async fn ordinary_quality_critic_blocks_oversized_files_without_functions() {
    let dir = tempfile::tempdir().unwrap();
    let files = vec![(
        "types.ts".into(),
        "// Data contract documentation\n".repeat(1001),
    )];
    let report = quality(&NoModel, &Config::from_env(), dir.path(), &files, options()).await;
    assert_eq!(report.functions_analyzed, 0);
    assert_eq!(report.approval, Some(false));
    assert_eq!(report.quality_score, None);
    let organization = report.organization.unwrap();
    assert!(
        organization
            .violations
            .iter()
            .any(|f| f.code == "file_line_limit")
    );
    assert!(report.reasons.iter().any(|r| r.contains("file_line_limit")));
}

#[tokio::test]
async fn ordinary_quality_critic_loads_repository_policy_and_rejects_malformed_policy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".kritic")).unwrap();
    let path = dir.path().join(".kritic/repository.toml");
    std::fs::write(
        &path,
        "version=1\n[organization]\nmax_file_lines=1\nhard_file_lines=2\n",
    )
    .unwrap();
    let files = vec![("types.ts".into(), "a\nb\nc\n".into())];
    let configured = quality(&NoModel, &Config::from_env(), dir.path(), &files, options()).await;
    assert_eq!(configured.approval, Some(false));
    assert!(
        configured
            .organization
            .unwrap()
            .violations
            .iter()
            .any(|f| f.code == "file_line_limit")
    );
    std::fs::write(&path, "version=1\n[organization]\nunknown=true\n").unwrap();
    let malformed = quality(&NoModel, &Config::from_env(), dir.path(), &files, options()).await;
    assert_eq!(malformed.approval, Some(false));
    assert!(malformed.organization.is_none());
    assert!(
        malformed
            .reasons
            .iter()
            .any(|r| r.contains("organization policy failed"))
    );
}

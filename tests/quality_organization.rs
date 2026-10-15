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

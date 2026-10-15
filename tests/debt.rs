use kritic::debt::{Dependency, Ecosystem, ReleaseInfo, Request, rank_dependency};
use std::collections::BTreeMap;

fn dependency(current: Option<&str>) -> Dependency {
    Dependency {
        ecosystem: Ecosystem::Cargo,
        name: "example".into(),
        requirement: Some("^1".into()),
        current: current.map(str::to_owned),
        scope: "direct".into(),
        manifest: "Cargo.toml".into(),
        public_registry: true,
    }
}
fn release(current: Option<&str>, latest: &str) -> ReleaseInfo {
    ReleaseInfo {
        ecosystem: Ecosystem::Cargo,
        name: "example".into(),
        current: current.map(str::to_owned),
        latest: Some(latest.into()),
        source_url: Some("https://crates.io/api/v1/crates/example".into()),
        metadata_sources: vec![],
        observed_at: Some("2026-10-15T00:00:00Z".into()),
        cache_age_seconds: Some(0),
        status: "fetched".into(),
        current_deprecated: Some(false),
        latest_deprecated: Some(false),
        deprecation_reason: None,
        requires_runtime: None,
        peer_dependencies: BTreeMap::new(),
        advisory_ids: vec![],
        errors: vec![],
    }
}

#[test]
fn update_effort_and_urgency_are_independent_evidence_estimates() {
    let patch = rank_dependency(&dependency(Some("1.2.3")), &release(Some("1.2.3"), "1.2.4"));
    assert_eq!(patch.version_status, "update_candidate");
    assert_eq!(patch.effort.rating, Some(1));
    assert!(patch.effort.estimated && patch.effort.label.contains("not verified"));
    assert_eq!(patch.urgency, "low");
    let major = rank_dependency(&dependency(Some("1.2.3")), &release(Some("1.2.3"), "2.0.0"));
    assert_eq!(major.effort.rating, Some(4));
    assert_eq!(
        major.urgency, "medium",
        "a new major alone is not a security emergency"
    );
    let pre_one = rank_dependency(&dependency(Some("0.2.3")), &release(Some("0.2.3"), "0.3.0"));
    assert_eq!(pre_one.effort.rating, Some(4));
    let mut vulnerable = release(Some("1.2.3"), "1.2.4");
    vulnerable.advisory_ids.push("TEST-ADVISORY-1".into());
    let advisory = rank_dependency(&dependency(Some("1.2.3")), &vulnerable);
    assert_eq!(advisory.urgency, "high");
    assert!(advisory.urgency_reason.contains("severity/exploitability"));
}

#[test]
fn newer_installed_qualified_unresolved_and_stale_versions_never_become_false_passes() {
    assert_eq!(
        rank_dependency(&dependency(Some("2.0.0")), &release(Some("2.0.0"), "1.9.9"))
            .version_status,
        "ahead_of_observed_default"
    );
    assert_eq!(
        rank_dependency(
            &dependency(Some("1.0.0-rc1")),
            &release(Some("1.0.0-rc1"), "1.0.0")
        )
        .version_status,
        "different_versions_unordered"
    );
    assert_eq!(
        rank_dependency(&dependency(None), &release(None, "1.0.0"))
            .effort
            .rating,
        None
    );
    let mut cached = release(Some("1.0.0"), "1.0.1");
    cached.status = "stale_cache".into();
    cached.errors.push("current metadata unavailable".into());
    let item = rank_dependency(&dependency(Some("1.0.0")), &cached);
    assert_eq!(item.version_status, "cached_update_candidate");
    assert!(item.urgency_reason.contains("stale") && item.urgency_reason.contains("partial"));
    cached.latest_deprecated = Some(true);
    assert!(
        rank_dependency(&dependency(Some("1.0.0")), &cached)
            .effort
            .rating
            .is_none()
    );
}

#[tokio::test]
async fn disabled_and_refresh_without_online_fail_before_reading_project_or_creating_cache() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = kritic::config::Config::load();
    cfg.runs_dir = temp.path().join("never-created");
    cfg.capabilities.insert("debt".into(), false);
    let request = || Request {
        base_dir: "missing-project".into(),
        online: false,
        refresh: false,
        source_paths: vec![],
    };
    assert!(
        kritic::debt::execute(&cfg, request())
            .await
            .unwrap_err()
            .contains("disabled")
    );
    cfg.capabilities.insert("debt".into(), true);
    cfg.disabled_tools
        .retain(|name| name != "technical_debt_critic");
    let mut refresh = request();
    refresh.refresh = true;
    assert!(
        kritic::debt::execute(&cfg, refresh)
            .await
            .unwrap_err()
            .contains("online")
    );
    assert!(!cfg.runs_dir.exists());
    let schema = schemars::schema_for!(Request);
    assert_eq!(schema.as_value()["type"], "object");
}

#[tokio::test]
async fn actual_offline_report_retains_missing_lookups_and_maintenance_evidence_without_approval() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("repo");
    std::fs::create_dir_all(base.join(".kritic")).unwrap();
    std::fs::write(
        base.join(".kritic/debt.toml"),
        r#"version=1
[[packages]]
ecosystem='npm'
name='fixture-package'
current='1.0.0'
public_registry=true
scope='direct'
[[maintenance]]
id='old-service'
summary='Replace the unsupported internal service adapter'
urgency='medium'
effort=3
source_url='https://example.invalid/migration'
due_date='2000-01-01'
"#,
    )
    .unwrap();
    std::fs::write(
        base.join("source.rs"),
        "// TODO: replace the deprecated facade\nlet literal=\"TODO not a comment\";\n",
    )
    .unwrap();
    let report = kritic::debt::review(
        &base,
        &temp.path().join("cache"),
        false,
        false,
        &["source.rs".into()],
    )
    .await
    .unwrap();
    assert!(report.dependencies_count > 0 && report.items_count >= 3);
    assert!(
        report
            .items
            .iter()
            .any(|item| item.package.as_deref() == Some("fixture-package")
                && item.lookup.as_ref().unwrap().status == "offline_missing")
    );
    assert!(
        report
            .items
            .iter()
            .any(|item| item.category == "declared_maintenance" && item.urgency == "high")
    );
    assert_eq!(
        report
            .items
            .iter()
            .filter(|item| item.category == "maintenance_comment")
            .count(),
        1
    );
    assert!(!report.coverage_gaps.is_empty());
    let json = serde_json::to_value(report).unwrap();
    assert!(json.get("approval").is_none());
    assert!(
        !temp.path().join("cache").exists(),
        "offline cache misses must not create a fake cache"
    );
}

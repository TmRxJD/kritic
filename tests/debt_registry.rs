//! Public offline surface: nothing is installed and private package names never leave the host.
use kritic::debt::{Dependency, Ecosystem, registry::lookup_all};

#[tokio::test]
async fn missing_offline_and_nonpublic_metadata_are_never_up_to_date() {
    let directory = tempfile::tempdir().unwrap();
    let dependencies = vec![
        Dependency {
            ecosystem: Ecosystem::Npm,
            name: "public-example".into(),
            requirement: Some("^1".into()),
            current: Some("1.0.0".into()),
            scope: "direct".into(),
            manifest: "package.json".into(),
            public_registry: true,
        },
        Dependency {
            ecosystem: Ecosystem::Npm,
            name: "private-do-not-publish".into(),
            requirement: None,
            current: None,
            scope: "direct".into(),
            manifest: "package.json".into(),
            public_registry: false,
        },
    ];
    let reports = lookup_all(&dependencies, directory.path(), false, false).await;
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].status, "offline_missing");
    assert_eq!(reports[1].status, "nonpublic_skipped");
    assert!(
        reports
            .iter()
            .all(|report| report.latest.is_none() && !report.errors.is_empty())
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

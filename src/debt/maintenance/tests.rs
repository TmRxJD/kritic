use super::*;

fn item() -> DebtItem {
    DebtItem {
        id: "fixture".into(),
        category: "dependency".into(),
        summary: "fixture dependency".into(),
        urgency: "medium".into(),
        urgency_reason: "fixture".into(),
        ecosystem: Some(Ecosystem::Cargo),
        package: Some("example".into()),
        current: Some("1.0.0".into()),
        latest: Some("2.0.0".into()),
        version_status: "update_candidate".into(),
        source: "Cargo.toml".into(),
        source_line: None,
        effort: estimate(None, "Unverified"),
        lookup: None,
    }
}

fn policy(root: &Path, variable: &str) {
    std::fs::create_dir_all(root.join(".kritic")).unwrap();
    std::fs::write(
        root.join(".kritic/debt.toml"),
        format!(
            r#"version=1
[[migrations]]
ecosystem='cargo'
name='example'
from='1.0.0'
to='2.0.0'
effort=3
summary='Enable the new service configuration mode'
source_url='https://example.invalid/migration'
required_env_vars=['{variable}']
"#
        ),
    )
    .unwrap();
}

#[test]
fn declared_environment_changes_only_apply_to_the_exact_version_pair() {
    let temp = tempfile::tempdir().unwrap();
    policy(temp.path(), "REQUIRED_MODE");
    let mut items = vec![item()];
    let mut gaps = vec![];
    apply(temp.path(), &[], &mut items, &mut gaps).unwrap();
    assert_eq!(items[0].effort.rating, Some(3));
    assert!(
        items[0].effort.estimated
            && items[0]
                .effort
                .evidence
                .iter()
                .any(|e| e.contains("REQUIRED_MODE"))
    );
    let mut other = item();
    other.latest = Some("2.1.0".into());
    let mut items = vec![other];
    let mut gaps = vec![];
    apply(temp.path(), &[], &mut items, &mut gaps).unwrap();
    assert_eq!(items[0].effort.rating, None);
    assert!(gaps.iter().any(|gap| gap.contains("not applied")));
}

#[test]
fn environment_values_and_outside_source_paths_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    policy(temp.path(), "REQUIRED_MODE=private-value");
    assert!(
        apply(temp.path(), &[], &mut vec![item()], &mut vec![])
            .unwrap_err()
            .contains("never values")
    );
    let other = tempfile::tempdir().unwrap();
    assert!(
        apply(
            other.path(),
            &["../outside.rs".into()],
            &mut vec![],
            &mut vec![]
        )
        .unwrap_err()
        .contains("traversal")
    );
}

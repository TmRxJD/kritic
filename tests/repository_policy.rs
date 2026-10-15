use std::{path::Path, process::Command};

fn git(base: &Path, args: &[&str]) {
    let result = Command::new("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init"]);
    std::fs::create_dir(dir.path().join(".kritic")).unwrap();
    std::fs::write(dir.path().join(".kritic/repository.toml"), "version=1\n").unwrap();
    git(dir.path(), &["add", ".kritic/repository.toml"]);
    dir
}

#[test]
fn staged_policy_ignores_unstaged_weakening_and_requires_fresh_plan() {
    let dir = fixture();
    std::fs::write(dir.path().join("code.rs"), "fn main() {}\n").unwrap();
    git(dir.path(), &["add", "code.rs"]);
    let names = vec![".kritic/repository.toml".into(), "code.rs".into()];
    kritic::commit_policy::plan(dir.path(), "Add explicit policy enforcement", &names).unwrap();
    std::fs::write(
        dir.path().join(".kritic/repository.toml"),
        "version=1\n[commit]\nrequire_scope_plan=false\n",
    )
    .unwrap();
    let result = kritic::repository::check(dir.path(), true).unwrap();
    assert!(result.approval);
    assert_eq!(result.snapshot, "staged_index");
    assert_eq!(result.files_analyzed, 1);
    std::fs::write(dir.path().join("code.rs"), "fn other() {}\n").unwrap();
    git(dir.path(), &["add", "code.rs"]);
    assert!(
        !kritic::repository::check(dir.path(), true)
            .unwrap()
            .approval
    );
}

#[test]
fn missing_invalid_and_unknown_policy_fields_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    assert!(kritic::repository::load(dir.path(), false).is_err());
    std::fs::create_dir(dir.path().join(".kritic")).unwrap();
    for text in [
        "version=2",
        "version=1\nunknown=true",
        "version=1\n[organization]\nhard_file_lines=0",
    ] {
        std::fs::write(dir.path().join(".kritic/repository.toml"), text).unwrap();
        assert!(kritic::repository::check(dir.path(), false).is_err());
    }
}

#[test]
fn bundled_policy_example_remains_valid() {
    let policy: kritic::repository::RepositoryPolicy =
        toml::from_str(include_str!("../examples/repository.toml")).unwrap();
    assert_eq!(policy.version, 1);
    kritic::organization::validate(&policy.organization).unwrap();
}

#[test]
fn empty_full_audit_is_not_a_pass_but_documentation_only_commits_are_valid() {
    let dir = fixture();
    let report = kritic::repository::check(dir.path(), false).unwrap();
    assert!(!report.approval);
    assert!(!report.source_coverage_errors.is_empty());
    kritic::commit_policy::plan(
        dir.path(),
        "Declare repository policy conventions",
        &[".kritic/repository.toml".into()],
    )
    .unwrap();
    assert!(
        kritic::repository::check(dir.path(), true)
            .unwrap()
            .approval
    );
}

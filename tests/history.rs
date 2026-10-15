use kritic::history::Store;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn git(base: &Path, args: &[&str]) {
    let output = kritic::proc::hidden_command("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init"]);
    git(
        repo.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(repo.path(), &["config", "user.name", "History test"]);
    std::fs::write(repo.path().join("source.rs"), "fn original() {}\n").unwrap();
    git(repo.path(), &["add", "source.rs"]);
    git(
        repo.path(),
        &["commit", "--no-verify", "-m", "Initial fixture"],
    );
    report(repo.path(), true);
    repo
}

fn report(base: &Path, approval: bool) {
    std::fs::write(base.join("report.json"), serde_json::to_vec(&json!({"tool":"repository_critic","approval":approval,"findings":[],"private":"raw-report-secret"})).unwrap()).unwrap();
}

fn inputs() -> Vec<PathBuf> {
    vec![PathBuf::from("source.rs")]
}
const CONTEXT: &str = "tool=repository_critic model=none command=repository-check config=v1";

#[test]
fn duplicate_snapshots_are_stable_and_mutated_inputs_are_visible() {
    let repo = repo();
    let storage = tempfile::tempdir().unwrap();
    let root = storage.path().join("history");
    let store = Store::new(&root);
    assert!(!root.exists());
    let first = store
        .record(
            repo.path(),
            &repo.path().join("report.json"),
            &inputs(),
            CONTEXT,
        )
        .unwrap();
    let duplicate = store
        .record(
            repo.path(),
            &repo.path().join("report.json"),
            &inputs(),
            CONTEXT,
        )
        .unwrap();
    assert_eq!(first.id, duplicate.id);
    assert_eq!(first.created_at, duplicate.created_at);
    std::fs::write(repo.path().join("source.rs"), "fn changed() {}\n").unwrap();
    let changed = store
        .record(
            repo.path(),
            &repo.path().join("report.json"),
            &inputs(),
            CONTEXT,
        )
        .unwrap();
    assert_ne!(changed.id, first.id);
    let comparison = store.compare(repo.path(), &first.id, &changed.id).unwrap();
    assert!(comparison.comparable);
    assert_eq!(comparison.changed_inputs, ["source.rs"]);
    assert!(comparison.differences.is_empty());
    assert!(comparison.historical_unverified);
    assert_eq!(store.list(repo.path()).unwrap().len(), 2);
    assert!(first.summary().historical_unverified);
}

#[test]
fn report_changes_have_bounded_hash_differences_and_context_mismatch_is_not_comparable() {
    let repo = repo();
    let storage = tempfile::tempdir().unwrap();
    let store = Store::new(storage.path());
    let first = store
        .record(
            repo.path(),
            &repo.path().join("report.json"),
            &inputs(),
            CONTEXT,
        )
        .unwrap();
    report(repo.path(), false);
    let changed = store
        .record(
            repo.path(),
            &repo.path().join("report.json"),
            &inputs(),
            CONTEXT,
        )
        .unwrap();
    let diff = store.compare(repo.path(), &first.id, &changed.id).unwrap();
    assert_eq!(diff.differences.len(), 1);
    assert_eq!(diff.differences[0].pointer, "/approval");
    assert_ne!(
        diff.differences[0].left_sha256,
        diff.differences[0].right_sha256
    );
    let other = store
        .record(
            repo.path(),
            &repo.path().join("report.json"),
            &inputs(),
            "tool=repository_critic model=other command=repository-check config=v1",
        )
        .unwrap();
    let comparison = store.compare(repo.path(), &first.id, &other.id).unwrap();
    assert!(!comparison.comparable);
    assert!(comparison.differences.is_empty());
    let values = |value| {
        Value::Object(
            (0..150)
                .map(|i| (format!("field{i:03}"), json!(value)))
                .chain([("tool".into(), json!("repository_critic"))])
                .collect(),
        )
    };
    std::fs::write(repo.path().join("report.json"), values(0).to_string()).unwrap();
    let left = store
        .record(
            repo.path(),
            &repo.path().join("report.json"),
            &inputs(),
            CONTEXT,
        )
        .unwrap();
    std::fs::write(repo.path().join("report.json"), values(1).to_string()).unwrap();
    let right = store
        .record(
            repo.path(),
            &repo.path().join("report.json"),
            &inputs(),
            CONTEXT,
        )
        .unwrap();
    let diff = store.compare(repo.path(), &left.id, &right.id).unwrap();
    assert_eq!(diff.differences_returned, 100);
    assert!(diff.differences_truncated);
}

#[test]
fn revisions_and_repository_identity_are_separate() {
    let first_repo = repo();
    let second_repo = repo();
    let storage = tempfile::tempdir().unwrap();
    let store = Store::new(storage.path());
    let first = store
        .record(
            first_repo.path(),
            &first_repo.path().join("report.json"),
            &inputs(),
            CONTEXT,
        )
        .unwrap();
    assert!(store.list(second_repo.path()).unwrap().is_empty());
    assert!(
        store
            .compare(second_repo.path(), &first.id, &first.id)
            .is_err()
    );
    git(
        first_repo.path(),
        &[
            "commit",
            "--allow-empty",
            "--no-verify",
            "-m",
            "Change revision only",
        ],
    );
    let changed = store
        .record(
            first_repo.path(),
            &first_repo.path().join("report.json"),
            &inputs(),
            CONTEXT,
        )
        .unwrap();
    assert!(
        store
            .compare(first_repo.path(), &first.id, &changed.id)
            .unwrap()
            .revision_changed
    );
}

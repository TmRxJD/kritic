use kritic::commit_policy::{CommitPolicy, check, plan, source_files};
use std::path::Path;

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

#[test]
fn index_content_and_tree_bound_scope_cannot_be_replaced_by_unstaged_work() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    git(base, &["init"]);
    git(base, &["config", "core.autocrlf", "false"]);
    std::fs::write(base.join("code.rs"), "fn staged() {}\n").unwrap();
    git(base, &["add", "code.rs"]);
    std::fs::write(base.join("code.rs"), "fn unstaged() {}\n").unwrap();
    assert_eq!(source_files(base, true).unwrap()[0].1, "fn staged() {}\n");
    plan(base, "Add staged source validation", &["code.rs".into()]).unwrap();
    assert!(check(base, &CommitPolicy::default()).unwrap().approval);
    git(base, &["add", "code.rs"]);
    assert!(!check(base, &CommitPolicy::default()).unwrap().approval);
}

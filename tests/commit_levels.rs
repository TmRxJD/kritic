use kritic::commit_policy::{CommitLevel, CommitPolicy, check, plan};
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

fn stage_lines(base: &Path, lines: usize) {
    let source: String = (0..lines)
        .map(|i| format!("// Fixture line {i}\n"))
        .collect();
    std::fs::write(base.join("fixture.rs"), source).unwrap();
    git(base, &["add", "fixture.rs"]);
    plan(
        base,
        "Add the review budget fixture",
        &["fixture.rs".into()],
    )
    .unwrap();
}

#[test]
fn presets_enforce_distinct_budgets_against_the_same_staged_tree() {
    let repo = tempfile::tempdir().unwrap();
    let base = repo.path();
    git(base, &["init"]);
    stage_lines(base, 300);
    let atomic = check(base, &CommitPolicy::for_level(CommitLevel::Atomic)).unwrap();
    let focused = check(base, &CommitPolicy::default()).unwrap();
    assert!(!atomic.approval);
    assert!(focused.approval);
    assert_eq!(atomic.level, CommitLevel::Atomic);
    assert_eq!(atomic.limits.max_changed_lines, 200);
    assert_eq!(atomic.staged_tree, focused.staged_tree);
    stage_lines(base, 900);
    assert!(!check(base, &CommitPolicy::default()).unwrap().approval);
    let broad = check(base, &CommitPolicy::for_level(CommitLevel::Broad)).unwrap();
    assert!(broad.approval);
    assert_eq!(broad.limits.max_changed_lines, 1600);
}

#[test]
fn omitted_level_preserves_focused_and_explicit_limits_override_presets() {
    let focused: CommitPolicy = toml::from_str("").unwrap();
    assert_eq!(focused.level, CommitLevel::Focused);
    assert_eq!(focused.max_changed_lines, 800);
    let atomic: CommitPolicy = toml::from_str("level='atomic'\nmax_changed_lines=350\nmax_files=2\nmax_patch_bytes=999\nrequire_scope_plan=false").unwrap();
    assert_eq!(atomic.level, CommitLevel::Atomic);
    assert_eq!(atomic.max_changed_lines, 350);
    assert_eq!(atomic.max_files, 2);
    assert_eq!(atomic.max_patch_bytes, 999);
    assert!(!atomic.require_scope_plan);
    let roundtrip: CommitPolicy = toml::from_str(&toml::to_string(&atomic).unwrap()).unwrap();
    assert_eq!(roundtrip.level, atomic.level);
    assert_eq!(roundtrip.max_changed_lines, 350);
    for level in ["broad", "focused", "atomic"] {
        let policy: CommitPolicy = toml::from_str(&format!("level='{level}'")).unwrap();
        assert!(policy.require_scope_plan);
        assert!(!policy.forbidden_paths.is_empty());
    }
    assert!(toml::from_str::<CommitPolicy>("level='tiny'").is_err());
    assert!(toml::from_str::<CommitPolicy>("max_filez=1").is_err());
}

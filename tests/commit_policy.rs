use kritic::commit_policy::{
    CommitPolicy, check, install_hook, plan, source_files, source_files_with_exclusions,
};
use std::{path::Path, process::Command};

fn git(base: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init"]);
    git(
        repo.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(repo.path(), &["config", "user.name", "Policy test"]);
    git(repo.path(), &["config", "core.autocrlf", "false"]);
    repo
}

fn stage(base: &Path, name: &str, text: &str) {
    std::fs::write(base.join(name), text).unwrap();
    git(base, &["add", "--", name]);
}

#[test]
fn initial_commit_scope_is_exact_and_bound_to_index_not_worktree() {
    let repo = repo();
    let base = repo.path();
    stage(base, "a space.rs", "fn staged() {}\n");
    assert!(!check(base, &CommitPolicy::default()).unwrap().approval);
    plan(base, "Add the staged entry point", &["a space.rs".into()]).unwrap();
    std::fs::write(base.join("a space.rs"), "fn unstaged() {}\n").unwrap();
    let report = check(base, &CommitPolicy::default()).unwrap();
    assert!(report.approval, "{:?}", report.findings);
    assert_eq!(report.added_lines, 1);
    assert_eq!(source_files(base, true).unwrap()[0].1, "fn staged() {}\n");
    assert_eq!(
        source_files(base, false).unwrap()[0].1,
        "fn unstaged() {}\n"
    );
    git(base, &["add", "--", "a space.rs"]);
    assert!(
        check(base, &CommitPolicy::default())
            .unwrap()
            .findings
            .iter()
            .any(|v| v.contains("Stale"))
    );
}

#[test]
fn declaration_rejects_vacuity_and_uncovered_paths() {
    let repo = repo();
    let base = repo.path();
    stage(base, "main.rs", "fn main() {}\n");
    assert!(plan(base, "everything", &["main.rs".into()]).is_err());
    assert!(plan(base, "Implement the entry point", &["*".into()]).is_err());
    assert!(plan(base, "Implement the entry point", &[]).is_err());
    assert!(
        plan(
            base,
            "Implement the entry point",
            &["main.rs".into(), "other.rs".into()]
        )
        .is_err()
    );
}

#[test]
fn rename_and_delete_require_old_and_new_explicit_paths() {
    let repo = repo();
    let base = repo.path();
    stage(base, "old.rs", "fn first() {}\n");
    git(base, &["commit", "--no-verify", "-m", "Initial fixture"]);
    git(base, &["mv", "old.rs", "new name.rs"]);
    assert!(plan(base, "Rename the entry source", &["new name.rs".into()]).is_err());
    plan(
        base,
        "Rename the entry source",
        &["old.rs".into(), "new name.rs".into()],
    )
    .unwrap();
    let report = check(base, &CommitPolicy::default()).unwrap();
    assert!(report.approval, "{:?}", report.findings);
    assert_eq!(source_files(base, true).unwrap().len(), 1);
    git(base, &["commit", "--no-verify", "-m", "Rename fixture"]);
    git(base, &["rm", "--", "new name.rs"]);
    plan(base, "Remove the unused entry", &["new name.rs".into()]).unwrap();
    assert!(check(base, &CommitPolicy::default()).unwrap().approval);
    assert!(source_files(base, true).unwrap().is_empty());
}

#[test]
fn budgets_and_sensitive_paths_fail_without_model() {
    let repo = repo();
    let base = repo.path();
    stage(base, ".env", "TOKEN=example\n");
    plan(base, "Add a test credential", &[".env".into()]).unwrap();
    let policy = CommitPolicy {
        max_files: 0,
        max_changed_lines: 0,
        max_patch_bytes: 1,
        ..Default::default()
    };
    let findings = check(base, &policy).unwrap().findings;
    assert!(findings.iter().any(|v| v.contains("Forbidden")));
    assert!(findings.iter().any(|v| v.contains("max_files")));
    assert!(findings.iter().any(|v| v.contains("max_changed_lines")));
    assert!(findings.iter().any(|v| v.contains("max_patch_bytes")));
}

#[test]
fn compressed_binary_cannot_evade_byte_budget() {
    let repo = repo();
    let base = repo.path();
    std::fs::write(base.join("zeros.bin"), vec![0u8; 20_000]).unwrap();
    git(base, &["add", "zeros.bin"]);
    plan(base, "Add a binary fixture", &["zeros.bin".into()]).unwrap();
    let policy = CommitPolicy {
        max_patch_bytes: 10_000,
        ..Default::default()
    };
    let report = check(base, &policy).unwrap();
    assert_eq!(report.binary_blob_bytes, 20_000);
    assert!(!report.approval);
    assert!(
        report
            .findings
            .iter()
            .any(|v| v.contains("uncompressed binary"))
    );
}

#[test]
fn source_corpus_excludes_assets_and_generated_directories() {
    let repo = repo();
    let base = repo.path();
    stage(base, "main.rs", "fn main() {}\n");
    std::fs::create_dir(base.join("dist")).unwrap();
    stage(base, "dist/generated.js", "console.log(1)\n");
    std::fs::write(base.join("image.png"), [0, 1, 2]).unwrap();
    git(base, &["add", "image.png"]);
    let files = source_files(base, true).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].0, "main.rs");
}

#[test]
fn hook_preserves_existing_hook_and_honors_hooks_path() {
    let repo = repo();
    let base = repo.path();
    git(base, &["config", "core.hooksPath", ".hooks"]);
    std::fs::create_dir(base.join(".hooks")).unwrap();
    let hook = base.join(".hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho existing\n").unwrap();
    let exe = std::env::current_exe().unwrap();
    assert!(install_hook(base, &exe).unwrap_err().contains("preserved"));
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(base)
            .args(["config", "--get", "kritic.executable"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(&hook).unwrap(),
        "#!/bin/sh\necho existing\n"
    );
    std::fs::remove_file(&hook).unwrap();
    assert!(install_hook(base, &exe).unwrap().installed);
    assert!(!install_hook(base, &exe).unwrap().installed);
    assert_eq!(
        git(base, &["config", "--get", "core.hooksPath"]).trim(),
        ".hooks"
    );
    assert!(
        std::fs::read_to_string(hook)
            .unwrap()
            .contains("repository check --staged --base-dir")
    );
}

#[test]
fn installed_hook_is_portable_and_executable_setting_is_private() {
    let repo = repo();
    let base = repo.path();
    let exe = std::env::current_exe().unwrap();
    let report = install_hook(base, &exe).unwrap();
    let hook = std::fs::read_to_string(report.path).unwrap();
    assert!(hook.contains("git rev-parse --show-toplevel"));
    assert!(hook.contains("KRITIC_BIN"));
    assert!(hook.contains("git config --get kritic.executable"));
    assert!(!hook.contains(base.to_str().unwrap()));
    assert!(!hook.contains(exe.to_str().unwrap()));
    let expected = exe.canonicalize().unwrap().to_str().unwrap().to_owned();
    #[cfg(windows)]
    let expected = expected
        .strip_prefix("\\\\?\\")
        .unwrap_or(&expected)
        .replace('\\', "/");
    assert_eq!(
        git(base, &["config", "--local", "--get", "kritic.executable"]).trim(),
        expected
    );
}

#[test]
fn cloned_hook_fails_closed_without_a_local_executable() {
    let repo = repo();
    let base = repo.path();
    git(base, &["config", "core.hooksPath", ".hooks"]);
    install_hook(base, &std::env::current_exe().unwrap()).unwrap();
    std::fs::write(base.join(".gitattributes"), ".hooks/* text eol=lf\n").unwrap();
    git(base, &["add", ".gitattributes", ".hooks/pre-commit"]);
    git(
        base,
        &["commit", "--no-verify", "-m", "Share the portable hook"],
    );
    let outer = tempfile::tempdir().unwrap();
    let cloned = outer.path().join("clone");
    git(
        base,
        &[
            "clone",
            "--no-hardlinks",
            base.to_str().unwrap(),
            cloned.to_str().unwrap(),
        ],
    );
    assert_eq!(
        std::fs::read(base.join(".hooks/pre-commit")).unwrap(),
        std::fs::read(cloned.join(".hooks/pre-commit")).unwrap()
    );
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(&cloned)
            .args(["config", "--local", "--get", "kritic.executable"])
            .status()
            .unwrap()
            .success()
    );
    let result = Command::new("git")
        .arg("-C")
        .arg(&cloned)
        .args(["-c", "core.hooksPath=.hooks", "hook", "run", "pre-commit"])
        .env(
            "KRITIC_BIN",
            "kritic-fixture-deliberately-missing-executable",
        )
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "A missing executable must not silently approve a commit"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("kritic-fixture-deliberately-missing-executable"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn literal_pathspecs_are_not_globs() {
    let repo = repo();
    let base = repo.path();
    stage(base, "[a].rs", "fn exact() {}\n");
    assert_eq!(source_files(base, true).unwrap()[0].0, "[a].rs");
}

#[test]
fn explicit_source_exclusions_are_disclosed_and_literal() {
    let repo = repo();
    let base = repo.path();
    std::fs::create_dir(base.join("catalogs")).unwrap();
    stage(base, "catalogs/en.ts", "export const data = {};\n");
    stage(base, "catalogs_extra.ts", "export const data = {};\n");
    let inventory = source_files_with_exclusions(base, true, &["catalogs".into()]).unwrap();
    assert_eq!(inventory.files.len(), 1);
    assert_eq!(inventory.files[0].0, "catalogs_extra.ts");
    assert_eq!(inventory.excluded_paths, ["catalogs/en.ts"]);
    assert!(source_files_with_exclusions(base, true, &["*".into()]).is_err());
    assert!(source_files_with_exclusions(base, true, &["../outside".into()]).is_err());
}

#[test]
fn full_inventory_reports_bad_source_and_retains_good_source() {
    let repo = repo();
    let base = repo.path();
    stage(base, "good.rs", "fn good() {}\n");
    std::fs::write(base.join("bad.mjs"), [0xffu8]).unwrap();
    git(base, &["add", "bad.mjs"]);
    let inventory = source_files_with_exclusions(base, false, &[]).unwrap();
    assert_eq!(inventory.files.len(), 1);
    assert_eq!(inventory.files[0].0, "good.rs");
    assert_eq!(inventory.coverage_errors.len(), 1);
    assert!(inventory.coverage_errors[0].contains("bad.mjs"));
    assert!(inventory.coverage_errors[0].contains("not UTF-8"));
    assert!(source_files(base, false).is_err());
    assert!(
        source_files_with_exclusions(base, true, &[])
            .unwrap_err()
            .contains("not UTF-8")
    );
}

#[test]
fn valid_utf8_with_nul_is_audited_with_nonblocking_parser_warning() {
    let repo = repo();
    let base = repo.path();
    stage(
        base,
        "separator.mjs",
        "export const separator = `domain\0folder`.split('\0');\n",
    );
    for staged in [false, true] {
        let inventory = source_files_with_exclusions(base, staged, &[]).unwrap();
        assert_eq!(inventory.files.len(), 1);
        assert!(inventory.files[0].1.contains('\0'));
        assert!(inventory.coverage_errors.is_empty());
        assert_eq!(inventory.source_warnings.len(), 1);
        assert!(inventory.source_warnings[0].contains("separator.mjs"));
        assert!(inventory.source_warnings[0].contains("project parser"));
        assert_eq!(source_files(base, staged).unwrap().len(), 1);
    }
}

#[test]
fn binary_replacement_cannot_hide_a_large_prior_blob() {
    let repo = repo();
    let base = repo.path();
    std::fs::write(base.join("binary.bin"), vec![0u8; 20_000]).unwrap();
    git(base, &["add", "binary.bin"]);
    git(base, &["commit", "--no-verify", "-m", "Initial binary"]);
    std::fs::write(base.join("binary.bin"), [0u8; 2]).unwrap();
    git(base, &["add", "binary.bin"]);
    plan(base, "Reduce the binary fixture", &["binary.bin".into()]).unwrap();
    let report = check(
        base,
        &CommitPolicy {
            max_patch_bytes: 10_000,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.binary_blob_bytes, 20_000);
    assert!(!report.approval);
}

#[test]
fn unresolved_index_is_rejected() {
    use std::io::Write;
    use std::process::Stdio;
    let repo = repo();
    let base = repo.path();
    stage(base, "conflict.rs", "fn conflict() {}\n");
    let entry = git(base, &["ls-files", "--stage", "conflict.rs"]);
    let hash = entry.split_whitespace().nth(1).unwrap();
    git(base, &["update-index", "--force-remove", "conflict.rs"]);
    let mut child = Command::new("git")
        .arg("-C")
        .arg(base)
        .args(["update-index", "--index-info"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(format!("100644 {hash} 1\tconflict.rs\n100644 {hash} 2\tconflict.rs\n100644 {hash} 3\tconflict.rs\n").as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
    assert!(check(base, &CommitPolicy::default()).is_err());
    assert!(source_files(base, true).unwrap_err().contains("Unmerged"));
}

#[test]
fn linked_worktree_scope_plan_is_private_to_that_index() {
    let repo = repo();
    let base = repo.path();
    stage(base, "initial.rs", "fn initial() {}\n");
    git(base, &["commit", "--no-verify", "-m", "Initial fixture"]);
    let outer = tempfile::tempdir().unwrap();
    let linked = outer.path().join("linked");
    git(
        base,
        &[
            "worktree",
            "add",
            "-b",
            "policy-linked",
            linked.to_str().unwrap(),
        ],
    );
    stage(&linked, "linked.rs", "fn linked() {}\n");
    plan(&linked, "Add linked worktree source", &["linked.rs".into()]).unwrap();
    assert!(check(&linked, &CommitPolicy::default()).unwrap().approval);
    assert!(!base.join(".git/kritic-scope-plan.json").exists());
    let private = git(
        &linked,
        &["rev-parse", "--git-path", "kritic-scope-plan.json"],
    );
    assert!(private.contains("worktrees"));
}

#[test]
fn hook_install_from_subdirectory_uses_repository_hooks_path() {
    let repo = repo();
    let base = repo.path();
    std::fs::create_dir(base.join("sub")).unwrap();
    git(base, &["config", "core.hooksPath", ".hooks"]);
    install_hook(&base.join("sub"), &std::env::current_exe().unwrap()).unwrap();
    assert!(base.join(".hooks/pre-commit").is_file());
    assert!(!base.join("sub/.hooks/pre-commit").exists());
}

#[test]
fn staged_symlink_mode_is_rejected_without_creating_a_native_symlink() {
    let repo = repo();
    let base = repo.path();
    stage(base, "target.rs", "fn target() {}\n");
    let entry = git(base, &["ls-files", "--stage", "target.rs"]);
    let hash = entry.split_whitespace().nth(1).unwrap();
    git(
        base,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("120000,{hash},link.rs"),
        ],
    );
    let error = source_files(base, true).unwrap_err();
    assert!(error.contains("not a regular index blob"), "{error}");
}

#[cfg(unix)]
#[test]
fn working_source_cannot_escape_through_an_internal_symlink_directory() {
    let repo = repo();
    let base = repo.path();
    std::fs::create_dir(base.join("internal")).unwrap();
    stage(base, "internal/source.rs", "fn original() {}\n");
    let external = tempfile::tempdir().unwrap();
    std::fs::write(external.path().join("source.rs"), "fn outside() {}\n").unwrap();
    std::fs::remove_file(base.join("internal/source.rs")).unwrap();
    std::fs::remove_dir(base.join("internal")).unwrap();
    std::os::unix::fs::symlink(external.path(), base.join("internal")).unwrap();
    let error = source_files(base, false).unwrap_err();
    assert!(error.contains("outside the repository root"), "{error}");
}

use kritic::debt::{Ecosystem, inventory::collect};
use std::path::Path;

fn put(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn cargo_renames_lock_versions_and_nonpublic_sources_are_explicit() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    put(
        root,
        "Cargo.toml",
        "[package]\nname='root'\nversion='0.1.0'\n[dependencies]\nrenamed={package='serde',version='^1'}\nlocal={path='../local'}\nshared={workspace=true}\nprivate={version='1',registry='company'}\n",
    );
    put(
        root,
        "Cargo.lock",
        "version=3\n[[package]]\nname='serde'\nversion='1.0.200'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n[[package]]\nname='local'\nversion='0.2.0'\n",
    );
    let inventory = collect(root).unwrap();
    let serde = inventory
        .dependencies
        .iter()
        .find(|d| d.name == "serde" && d.scope == "direct")
        .unwrap();
    assert_eq!(serde.current.as_deref(), Some("1.0.200"));
    assert_eq!(serde.requirement.as_deref(), Some("^1"));
    assert!(serde.public_registry);
    for name in ["local", "shared", "private"] {
        assert!(
            !inventory
                .dependencies
                .iter()
                .find(|d| d.name == name && d.scope == "direct")
                .unwrap()
                .public_registry
        );
    }
}

#[test]
fn npm_aliases_workspace_resolution_and_registry_overrides_do_not_leak_credentials() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    put(
        root,
        "package.json",
        r#"{"private":true,"dependencies":{"foo":"^1","alias":"npm:actual@^2","@private/lib":"^3","local":"workspace:*","gitpkg":"github:owner/pkg"}}"#,
    );
    put(
        root,
        "apps/site/package.json",
        r#"{"dependencies":{"foo":"^2"}}"#,
    );
    put(
        root,
        ".npmrc",
        "@private:registry=https://user:secret@example.invalid/\nregistry=https://registry.npmjs.org/\n//example.invalid/:_authToken=VERY_SECRET_TOKEN\n",
    );
    put(
        root,
        "package-lock.json",
        r#"{"lockfileVersion":3,"packages":{"":{},"node_modules/foo":{"version":"1.2.3","resolved":"https://registry.npmjs.org/foo/-/foo-1.2.3.tgz"},"apps/site/node_modules/foo":{"version":"2.3.4","resolved":"https://registry.npmjs.org/foo/-/foo-2.3.4.tgz"},"node_modules/alias":{"name":"actual","version":"2.0.1","resolved":"https://registry.npmjs.org/actual/-/actual-2.0.1.tgz"},"node_modules/@private/lib":{"version":"3.0.0","resolved":"https://example.invalid/private.tgz"}}}"#,
    );
    let inventory = collect(root).unwrap();
    let root_foo = inventory
        .dependencies
        .iter()
        .find(|d| d.name == "foo" && d.manifest == "package.json")
        .unwrap();
    assert_eq!(root_foo.current.as_deref(), Some("1.2.3"));
    assert!(root_foo.public_registry);
    let app_foo = inventory
        .dependencies
        .iter()
        .find(|d| d.name == "foo" && d.manifest == "apps/site/package.json")
        .unwrap();
    assert_eq!(app_foo.current.as_deref(), Some("2.3.4"));
    assert!(
        inventory
            .dependencies
            .iter()
            .any(|d| d.name == "actual" && d.scope == "direct" && d.public_registry)
    );
    for name in ["@private/lib", "local", "gitpkg"] {
        assert!(
            !inventory
                .dependencies
                .iter()
                .find(|d| d.name == name && d.scope == "direct")
                .unwrap()
                .public_registry
        );
    }
    let serialized = serde_json::to_string(&inventory).unwrap();
    assert!(!serialized.contains("VERY_SECRET_TOKEN"));
    assert!(!serialized.contains("user:secret"));
}

#[test]
fn pnpm_v9_importer_versions_respect_workspaces_scoped_names_and_peers() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    put(
        root,
        "package.json",
        r#"{"dependencies":{"vue":"^3.5","@scope/ui":"^2","local":"workspace:*"}}"#,
    );
    put(
        root,
        "apps/web/package.json",
        r#"{"dependencies":{"vue":"^3.4"}}"#,
    );
    put(
        root,
        "pnpm-lock.yaml",
        "lockfileVersion: '9.0'\nimporters:\n  .:\n    dependencies:\n      vue: {specifier: '^3.5', version: '3.5.22(typescript@5.9.2)'}\n      '@scope/ui': {specifier: '^2', version: '2.1.0(vue@3.5.22)'}\n      local: {specifier: 'workspace:*', version: 'link:packages/local'}\n  apps/web:\n    dependencies:\n      vue: {specifier: '^3.4', version: '3.4.38'}\npackages:\n  vue@3.5.22: {resolution: {integrity: sha512-fixture}}\n  vue@3.4.38: {resolution: {integrity: sha512-fixture}}\n  '@scope/ui@2.1.0': {resolution: {integrity: sha512-fixture}}\n",
    );
    let inventory = collect(root).unwrap();
    assert_eq!(
        inventory
            .dependencies
            .iter()
            .find(|d| d.name == "vue" && d.manifest == "package.json")
            .unwrap()
            .current
            .as_deref(),
        Some("3.5.22")
    );
    assert_eq!(
        inventory
            .dependencies
            .iter()
            .find(|d| d.name == "vue" && d.manifest == "apps/web/package.json")
            .unwrap()
            .current
            .as_deref(),
        Some("3.4.38")
    );
    assert!(inventory.dependencies.iter().any(|d| d.name == "@scope/ui"
        && d.scope == "transitive"
        && d.current.as_deref() == Some("2.1.0")));
    assert!(
        !inventory
            .dependencies
            .iter()
            .find(|d| d.name == "local" && d.manifest == "package.json")
            .unwrap()
            .public_registry
    );
    assert!(
        !inventory
            .coverage_gaps
            .iter()
            .any(|gap| gap.contains("Unsupported pnpm"))
    );
}

#[test]
fn python_locks_normalize_names_and_ranges_are_not_current_versions() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    put(
        root,
        "pyproject.toml",
        "[project]\nname='example'\ndependencies=['Req_Core>=1','internal>=1']\n[tool.uv.sources]\ninternal={path='../internal'}\n",
    );
    put(
        root,
        "uv.lock",
        "version=1\n[[package]]\nname='req-core'\nversion='1.2.3'\nsource={registry='https://pypi.org/simple'}\n",
    );
    put(
        root,
        "requirements-dev.txt",
        "pinned==2.0.0\n-r other.txt\n",
    );
    let inventory = collect(root).unwrap();
    let dep = inventory
        .dependencies
        .iter()
        .find(|d| d.name == "req-core" && d.scope == "direct")
        .unwrap();
    assert_eq!(dep.current.as_deref(), Some("1.2.3"));
    assert_eq!(dep.ecosystem, Ecosystem::Pypi);
    assert!(
        !inventory
            .dependencies
            .iter()
            .find(|d| d.name == "internal")
            .unwrap()
            .public_registry
    );
    assert!(
        inventory
            .dependencies
            .iter()
            .find(|d| d.name == "pinned")
            .unwrap()
            .current
            .is_none()
    );
    assert!(
        inventory
            .coverage_gaps
            .iter()
            .any(|gap| gap.contains("option/include"))
    );
}

#[test]
fn malformed_and_unsupported_manifests_retain_valid_libraries() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    put(root, "Cargo.toml", "[broken");
    put(root, "yarn.lock", "# unsupported");
    put(root, "go.mod", "module example.invalid/project\n");
    put(root, "package.json", r#"{"dependencies":{"valid":"^1"}}"#);
    put(
        root,
        ".kritic/debt.toml",
        "version=1\n[maintenance]\npriority='review'\n[[packages]]\necosystem='go'\nname='example.org/mod'\ncurrent='v1.2.3'\nscope='direct'\npublic_registry=true\n[[packages]]\necosystem='maven'\nname='group:artifact'\ncurrent='^2.0'\nscope='direct'\npublic_registry=true\n",
    );
    let inventory = collect(root).unwrap();
    assert!(inventory.dependencies.iter().any(|d| d.name == "valid"));
    assert!(
        inventory
            .dependencies
            .iter()
            .any(|d| d.ecosystem == Ecosystem::Go && d.current.as_deref() == Some("v1.2.3"))
    );
    assert!(
        !inventory
            .dependencies
            .iter()
            .any(|d| d.name == "group:artifact")
    );
    for expected in ["Malformed Cargo", "yarn.lock", "go.mod", "Invalid explicit"] {
        assert!(
            inventory
                .coverage_gaps
                .iter()
                .any(|gap| gap.contains(expected)),
            "{expected}: {:?}",
            inventory.coverage_gaps
        );
    }
}

#[test]
fn corpus_is_manifest_only_and_bounds_are_reported() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    put(
        root,
        "node_modules/ignored/package.json",
        r#"{"dependencies":{"ignored":"1"}}"#,
    );
    put(
        root,
        "dist/package.json",
        r#"{"dependencies":{"ignored":"1"}}"#,
    );
    let inventory = collect(root).unwrap();
    assert!(inventory.dependencies.is_empty());
    assert!(
        inventory
            .coverage_gaps
            .iter()
            .any(|gap| gap.contains("No supported"))
    );
    put(root, "package.json", &" ".repeat(2_097_153));
    let inventory = collect(root).unwrap();
    assert!(
        inventory
            .coverage_gaps
            .iter()
            .any(|gap| gap.contains("2 MiB"))
    );
    std::fs::remove_file(root.join("package.json")).unwrap();
    for i in 0..201 {
        put(root, &format!("packages/p{i}/package.json"), "{}");
    }
    assert!(collect(root).unwrap_err().contains("200 manifests"));
}

#[test]
fn unknown_lock_versions_and_nonexact_resolutions_are_visible_gaps() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    put(root, "pnpm-lock.yaml", "lockfileVersion: '6.0'\n");
    put(
        root,
        "package-lock.json",
        r#"{"packages":{"node_modules/odd":{"version":"github:owner/project"}}}"#,
    );
    put(
        root,
        "package.json",
        r#"{"dependencies":{"odd":"github:owner/project"}}"#,
    );
    let inventory = collect(root).unwrap();
    assert!(inventory.dependencies.iter().all(|d| d.current.is_none()));
    assert!(
        inventory
            .coverage_gaps
            .iter()
            .any(|gap| gap.contains("Unsupported pnpm"))
    );
    assert!(
        inventory
            .coverage_gaps
            .iter()
            .any(|gap| gap.contains("nonexact"))
    );
}

#[test]
fn dependency_overflow_is_an_error_not_a_truncated_inventory() {
    let repo = tempfile::tempdir().unwrap();
    let dependencies: serde_json::Map<String, serde_json::Value> = (0..1001)
        .map(|index| (format!("package-{index}"), serde_json::json!("^1")))
        .collect();
    put(
        repo.path(),
        "package.json",
        &serde_json::json!({"dependencies":dependencies}).to_string(),
    );
    assert!(collect(repo.path()).unwrap_err().contains("1000 entries"));
}

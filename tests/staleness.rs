use std::collections::BTreeMap;
use std::path::Path;

use kritic::contract::hash;
use kritic::staleness::{Authority, Generated, Manifest, TestBinding, check, sync};

fn put(root: &Path, path: &str, bytes: impl AsRef<[u8]>) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, bytes).unwrap();
}

fn fixture() -> (tempfile::TempDir, Manifest) {
    let root = tempfile::tempdir().unwrap();
    let contract = b"{\"cases\":[{\"input\":0,\"expected\":1}]}";
    let test = b"assert_eq!(increment(0), independent_expected);";
    put(root.path(), "contracts/increment.json", contract);
    put(root.path(), "tests/increment.rs", test);
    let m = Manifest {
        version: 1,
        test_roots: vec!["tests".into()],
        test_pattern: r"^tests/[^/]+\.rs$".into(),
        authorities: vec![Authority {
            id: "increment".into(),
            domain: "increment behavior".into(),
            path: "contracts/increment.json".into(),
            owner: "core team".into(),
            purpose: "Independent input/output acceptance cases".into(),
        }],
        tests: vec![TestBinding {
            path: "tests/increment.rs".into(),
            purpose: "Checks implementation against independent cases".into(),
            reviewed_test_sha256: hash(test),
            reviewed_inputs: BTreeMap::from([("increment".into(), hash(contract))]),
            validation_command: "cargo test --test increment".into(),
        }],
        generated: vec![Generated {
            path: "fixtures/increment.json".into(),
            authority: "increment".into(),
            pointer: "/cases".into(),
            purpose: "Portable contract cases".into(),
            consumers: vec!["tests/increment.rs".into()],
        }],
        exclusions: vec![],
        boundaries: vec![],
        invariants: vec![],
    };
    save(root.path(), &m);
    (root, m)
}

fn save(root: &Path, m: &Manifest) {
    put(
        root,
        ".kritic/test-contracts.toml",
        toml::to_string(m).unwrap(),
    );
}

fn guarded_fixture() -> (tempfile::TempDir, Manifest) {
    use kritic::staleness::{Boundary, FailurePolicy, GuardKind, Invariant, NegativeTest};
    let (root, mut m) = fixture();
    let guard = b"fn validate(input: &str) -> Result<(), &'static str> { if input.is_empty() { return Err(\"empty\"); } Ok(()) }";
    let negative = b"fn rejects_empty() { assert!(validate(\"\").is_err()); }";
    put(root.path(), "src/input.rs", guard);
    put(root.path(), "tests/increment.rs", negative);
    m.tests[0].reviewed_test_sha256 = hash(negative);
    m.tests[0]
        .reviewed_inputs
        .insert("guard".into(), hash(guard));
    m.authorities.push(Authority {
        id: "guard".into(),
        domain: "input API".into(),
        path: "src/input.rs".into(),
        owner: "input team".into(),
        purpose: "Canonical input-validation boundary".into(),
    });
    m.boundaries.push(Boundary {
        id: "input".into(),
        authority: "guard".into(),
        risk: "External empty input could reach stateful processing".into(),
        external_input: true,
        required_in_release: true,
        required_invariants: vec!["nonempty".into()],
    });
    m.invariants.push(Invariant {
        id: "nonempty".into(),
        boundary: "input".into(),
        condition: "input length > 0 before processing".into(),
        rationale: "Empty input has no meaningful work item and must not mutate state".into(),
        guard_kind: GuardKind::RuntimeGuard,
        failure_policy: FailurePolicy::RejectError,
        enforcement_pattern: r#"if input\.is_empty\(\) \{ return Err\("empty"\); \}"#.into(),
        negative_tests: vec![NegativeTest {
            path: "tests/increment.rs".into(),
            pattern: r"fn rejects_empty\(\)".into(),
            purpose: "Empty input is rejected".into(),
        }],
    });
    save(root.path(), &m);
    sync(root.path(), None, true).unwrap();
    (root, m)
}

#[test]
fn invariant_consistency_retains_guard_evidence_without_semantic_approval() {
    let (root, _) = guarded_fixture();
    let report = kritic::staleness::check_invariants(root.path(), None).unwrap();
    assert!(report.deterministic_ok);
    assert!(report.approval.is_none());
    assert_eq!(report.invariant_evidence[0].enforcement_line, Some(1));
    assert!(
        report.invariant_evidence[0]
            .enforcement_excerpt
            .as_ref()
            .unwrap()
            .contains("return Err")
    );
    put(
        root.path(),
        "src/input.rs",
        "fn validate() { /* guard accidentally removed */ }",
    );
    assert!(codes(root.path()).contains(&"invariant_guard_missing".into()));
    assert!(codes(root.path()).contains(&"authority_changed".into()));
}

#[test]
fn missing_required_invariants_debug_release_guards_and_external_panics_are_rejected() {
    use kritic::staleness::{FailurePolicy, GuardKind};
    let (root, mut m) = guarded_fixture();
    m.boundaries[0]
        .required_invariants
        .push("uninstalled".into());
    m.invariants[0].guard_kind = GuardKind::DebugAssertion;
    m.invariants[0].failure_policy = FailurePolicy::PanicInternal;
    m.invariants[0].negative_tests.clear();
    save(root.path(), &m);
    let found = codes(root.path());
    for code in [
        "required_invariant_missing",
        "debug_only_invariant",
        "panic_on_external_input",
        "invariant_negative_test_missing",
    ] {
        assert!(found.contains(&code.into()), "{code}");
    }
}

#[test]
fn empty_invariant_inventory_and_vacuous_test_bindings_do_not_pass() {
    let (root, _) = fixture();
    assert!(
        !kritic::staleness::check_invariants(root.path(), None)
            .unwrap()
            .deterministic_ok
    );
    let (root, mut m) = guarded_fixture();
    m.invariants[0].enforcement_pattern = ".*".into();
    save(root.path(), &m);
    assert_eq!(check(root.path(), None).unwrap().status, "setup_required");
    m.invariants[0].enforcement_pattern = "input".into();
    m.invariants[0].negative_tests[0].pattern = "fn nonexistent_case".into();
    save(root.path(), &m);
    assert!(codes(root.path()).contains(&"invariant_negative_case_missing".into()));
}
fn codes(root: &Path) -> Vec<String> {
    check(root, None)
        .unwrap()
        .findings
        .into_iter()
        .map(|f| f.code)
        .collect()
}

#[test]
fn read_only_audit_exposes_fixture_drift_without_creating_or_approving_anything() {
    let (root, _) = fixture();
    assert!(codes(root.path()).contains(&"generated_fixture_unavailable".into()));
    assert!(!root.path().join("fixtures").exists());
    let dry = sync(root.path(), None, false).unwrap();
    assert_eq!(dry.would_update, ["fixtures/increment.json"]);
    assert!(dry.written.is_empty());
    sync(root.path(), None, true).unwrap();
    let report = check(root.path(), None).unwrap();
    assert!(report.deterministic_ok);
    assert_eq!((report.discovered_tests, report.mapped_tests), (1, 1));
    assert!(
        report.approval.is_none(),
        "Hash agreement cannot prove execution or semantic correctness"
    );
    assert_eq!(
        std::fs::read(root.path().join("fixtures/increment.json")).unwrap(),
        b"[\n  {\n    \"input\": 0,\n    \"expected\": 1\n  }\n]\n"
    );
}

#[test]
fn changing_contract_and_syncing_fixtures_cannot_silently_bless_the_tests() {
    let (root, _) = fixture();
    sync(root.path(), None, true).unwrap();
    let manifest = std::fs::read(root.path().join(".kritic/test-contracts.toml")).unwrap();
    put(
        root.path(),
        "contracts/increment.json",
        br#"{"cases":[{"input":0,"expected":2}]}"#,
    );
    assert!(codes(root.path()).contains(&"authority_changed".into()));
    assert!(codes(root.path()).contains(&"generated_fixture_drift".into()));
    assert!(sync(root.path(), None, true).unwrap().review_required);
    assert!(codes(root.path()).contains(&"authority_changed".into()));
    assert!(!codes(root.path()).contains(&"generated_fixture_drift".into()));
    assert_eq!(
        std::fs::read(root.path().join(".kritic/test-contracts.toml")).unwrap(),
        manifest
    );
}

#[test]
fn new_changed_deleted_and_unmapped_tests_remain_visible() {
    let (root, _) = fixture();
    put(root.path(), "tests/added.rs", "assert!(false);");
    put(
        root.path(),
        "tests/increment.rs",
        "assert!(true); // weakened test",
    );
    let found = codes(root.path());
    assert!(found.contains(&"unmapped_test".into()));
    assert!(found.contains(&"test_changed".into()));
    std::fs::remove_file(root.path().join("tests/increment.rs")).unwrap();
    assert!(codes(root.path()).contains(&"test_not_discovered".into()));
}

#[test]
fn incomplete_or_empty_discovery_never_passes() {
    let (root, mut m) = fixture();
    m.test_pattern = "^does-not-match$".into();
    save(root.path(), &m);
    assert!(codes(root.path()).contains(&"empty_inventory".into()));
    m.test_roots = vec!["missing".into()];
    save(root.path(), &m);
    assert!(codes(root.path()).contains(&"inventory_incomplete".into()));
}

#[test]
fn canonical_conflicts_self_oracles_and_vacuous_manifests_are_setup_errors() {
    let (root, mut m) = fixture();
    m.authorities.push(Authority {
        id: "other".into(),
        domain: "increment behavior".into(),
        path: "contracts/other.json".into(),
        owner: "someone".into(),
        purpose: "Conflicting owner".into(),
    });
    save(root.path(), &m);
    assert_eq!(check(root.path(), None).unwrap().status, "setup_required");
    m.authorities.pop();
    m.authorities[0].path = m.tests[0].path.clone();
    save(root.path(), &m);
    assert!(
        check(root.path(), None).unwrap().findings[0]
            .detail
            .contains("own canonical authority")
    );
    m.tests.clear();
    save(root.path(), &m);
    assert!(!check(root.path(), None).unwrap().deterministic_ok);
}

#[test]
fn sync_rejects_sources_tests_manifest_and_traversal_as_outputs() {
    let (root, mut m) = fixture();
    for path in [
        "contracts/increment.json",
        "tests/increment.rs",
        ".kritic/test-contracts.toml",
        "../escape.json",
        "C:/escape.json",
        ".git/config",
    ] {
        m.generated[0].path = path.into();
        save(root.path(), &m);
        assert!(sync(root.path(), None, true).is_err(), "{path}");
    }
    assert_eq!(
        std::fs::read(root.path().join("tests/increment.rs")).unwrap(),
        b"assert_eq!(increment(0), independent_expected);"
    );
}

#[test]
fn sync_validates_all_sources_before_any_write() {
    let (root, mut m) = fixture();
    m.generated.push(Generated {
        path: "fixtures/other.json".into(),
        authority: "increment".into(),
        pointer: "/absent".into(),
        purpose: "Bad projection".into(),
        consumers: vec!["tests/increment.rs".into()],
    });
    save(root.path(), &m);
    assert!(sync(root.path(), None, true).is_err());
    assert!(!root.path().join("fixtures").exists());
}

#[test]
fn excluded_tests_are_not_reported_current_and_stale_exclusions_are_detected() {
    let (root, mut m) = fixture();
    put(root.path(), "tests/legacy.rs", "old test");
    m.exclusions.push(kritic::staleness::Exclusion {
        path: "tests/legacy.rs".into(),
        reason: "Retirement review pending".into(),
    });
    save(root.path(), &m);
    let report = check(root.path(), None).unwrap();
    assert_eq!(report.excluded_tests, 1);
    assert!(!report.deterministic_ok);
    std::fs::remove_file(root.path().join("tests/legacy.rs")).unwrap();
    assert!(codes(root.path()).contains(&"obsolete_exclusion".into()));
}

#[test]
fn oversized_inputs_and_unknown_fields_cannot_be_reported_current() {
    let (root, _) = fixture();
    put(
        root.path(),
        "contracts/increment.json",
        vec![b' '; 1024 * 1024 + 1],
    );
    assert!(codes(root.path()).contains(&"authority_unavailable".into()));
    put(
        root.path(),
        ".kritic/test-contracts.toml",
        "version=1\nignore_all=true",
    );
    assert_eq!(check(root.path(), None).unwrap().status, "setup_required");
}

#[test]
fn untrusted_manifest_cannot_request_commands_and_cli_has_explicit_write_boundary() {
    use clap::Parser;
    assert!(
        serde_json::from_value::<kritic::staleness::Request>(
            serde_json::json!({"base_dir":".","write":true})
        )
        .is_err()
    );
    assert!(
        kritic::cli::Cli::try_parse_from([
            "kritic",
            "staleness",
            "sync",
            "--base-dir",
            ".",
            "--write"
        ])
        .is_ok()
    );
    assert!(kritic::cli::Cli::try_parse_from(["kritic", "staleness", "check", "--write"]).is_err());
    assert_eq!(kritic::staleness::schema()["type"], "object");
}

#[test]
fn both_optional_critics_construct_real_mcp_schemas_without_model_dependencies() {
    let mut cfg = kritic::config::Config::load();
    cfg.backend = "off".into();
    cfg.vision_backend = "off".into();
    cfg.disabled_tools.clear();
    for name in ["staleness", "invariants"] {
        cfg.capabilities.insert(name.into(), true);
    }
    assert!(cfg.tool_enabled("staleness_critic") && cfg.tool_enabled("invariant_critic"));
    let _server = kritic::server::Kritic::new(cfg);
    assert_eq!(
        schemars::schema_for!(kritic::staleness::Request).as_value()["type"],
        "object"
    );
}

#[tokio::test]
async fn work_contract_uses_the_same_graph_and_cannot_hide_drift() {
    let (root, _) = fixture();
    sync(root.path(), None, true).unwrap();
    put(
        root.path(),
        ".kritic/contract.toml",
        "version=1\ntitle='Test freshness'\n[[requirements]]\nid='currency'\ndescription='Declared canonical test currency'\n[[requirements.checks]]\nkind='test_freshness'\npath='.kritic/test-contracts.toml'\n",
    );
    let cfg = kritic::config::Config::load();
    let good = kritic::contract::critique(&cfg, root.path(), None, "freshness", false).await;
    assert_eq!(
        good.requirements[0].outcome,
        kritic::contract::Outcome::Verified
    );
    assert!(
        good.requirements[0].checks[0]
            .detail
            .contains("execution remain unverified")
    );
    put(root.path(), "contracts/increment.json", "{}");
    let bad = kritic::contract::critique(&cfg, root.path(), None, "freshness", false).await;
    assert_eq!(bad.approval, Some(false));
}

#[cfg(unix)]
#[test]
fn symlink_sources_and_outputs_cannot_escape_the_repository() {
    let (root, mut m) = fixture();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
    m.generated[0].path = "linked/output.json".into();
    save(root.path(), &m);
    assert!(sync(root.path(), None, true).is_err());
    assert!(!outside.path().join("output.json").exists());
}

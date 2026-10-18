use kritic::framing::{Report, Request, execute};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn fixture() -> (tempfile::TempDir, Value) {
    let root = tempfile::tempdir().unwrap();
    let case: Value = serde_json::from_str(include_str!("../examples/debug-case.json")).unwrap();
    for evidence in case["evidence"].as_array().unwrap() {
        let relative = evidence["path"].as_str().unwrap();
        let bytes = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)).unwrap();
        assert_eq!(
            hash(&bytes),
            evidence["sha256"].as_str().unwrap(),
            "Example hash drift: {relative}"
        );
        let target = root.path().join(relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
    (root, case)
}

fn run(root: &Path, case: &Value) -> Result<Report, String> {
    std::fs::create_dir_all(root.join(".kritic")).unwrap();
    std::fs::write(
        root.join(".kritic/debug-case.json"),
        serde_json::to_vec(case).unwrap(),
    )
    .unwrap();
    execute(&Request {
        base_dir: root.to_str().unwrap().into(),
        case_path: None,
    })
}

fn has_finding(report: &Report, code: &str) -> bool {
    report.findings.iter().any(|finding| finding.code == code)
}

/// Repinning isolates semantic assertion/context checks from the independent stale-byte check.
fn mutate_observation(root: &Path, case: &mut Value, id: &str, change: impl FnOnce(&mut Value)) {
    let index = case["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .position(|item| item["id"] == id)
        .unwrap();
    let path = root.join(case["evidence"][index]["path"].as_str().unwrap());
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    change(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap();
    std::fs::write(path, &bytes).unwrap();
    case["evidence"][index]["sha256"] = json!(hash(&bytes));
}

fn add_observation(root: &Path, case: &mut Value, id: &str, data: Value) {
    let value = json!({"context":{"revision":case["revision"],"run_id":case["run_id"],"input_digest":case["input_digest"]},"data":data});
    let bytes = serde_json::to_vec(&value).unwrap();
    let path = format!("examples/framing-case/{id}.json");
    std::fs::write(root.join(&path), &bytes).unwrap();
    let assertions: Vec<Value> = value["data"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| json!({"pointer":format!("/data/{key}"),"expected":value}))
        .collect();
    case["evidence"].as_array_mut().unwrap().push(json!({"id":id,"kind":"observation","path":path,"sha256":hash(&bytes),"assertions":assertions}));
}

#[test]
fn one_supported_cause_is_a_consistent_case_never_root_cause_attestation() {
    let (root, case) = fixture();
    let report = run(root.path(), &case).unwrap();
    assert_eq!(
        report.status,
        "consistent",
        "{}",
        serde_json::to_string_pretty(&report).unwrap()
    );
    assert!(report.findings.is_empty());
    assert_eq!(report.evidence.len(), 4);
    assert!(
        report
            .evidence
            .iter()
            .all(|item| item.verified && item.state == "verified")
    );
    let value = serde_json::to_value(&report).unwrap();
    assert!(value.get("approval").is_none());
    assert!(
        report
            .review_packet
            .to_string()
            .contains("discarded-canonical-setting")
    );
    assert!(
        report
            .review_packet
            .to_string()
            .contains("client_timeout_ms")
    );
}

#[test]
fn changing_actual_observation_values_refutes_unchanged_content_assertions() {
    let (root, mut case) = fixture();
    mutate_observation(root.path(), &mut case, "failure", |value| {
        value["data"]["client_timeout_ms"] = json!(1500)
    });
    let report = run(root.path(), &case).unwrap();
    assert_eq!(report.status, "contradicted");
    let failure = report
        .evidence
        .iter()
        .find(|item| item.id == "failure")
        .unwrap();
    assert_eq!(failure.state, "contradicted");
    assert!(!failure.verified);
    assert!(failure.detail.contains("/data/client_timeout_ms"));
}

#[test]
fn changed_bytes_are_stale_even_if_the_case_has_all_required_fields() {
    let (root, case) = fixture();
    let path = root.path().join("examples/framing-case/client.rs");
    std::fs::write(
        path,
        "pub fn client_timeout_ms(canonical_ms: u64) -> u64 { canonical_ms }\n",
    )
    .unwrap();
    let report = run(root.path(), &case).unwrap();
    assert_eq!(report.status, "stale");
    assert!(
        report
            .evidence
            .iter()
            .any(|item| item.id == "client-source" && item.state == "stale")
    );
}

#[test]
fn repinned_observation_from_another_context_is_still_stale() {
    for field in ["revision", "run_id", "input_digest"] {
        let (root, mut case) = fixture();
        mutate_observation(root.path(), &mut case, "failure", |value| {
            value["context"][field] = json!("another-investigation")
        });
        let report = run(root.path(), &case).unwrap();
        assert_eq!(report.status, "stale", "Mismatched {field} was accepted");
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.id == "failure" && !item.verified)
        );
    }
}

#[test]
fn conflicting_scattered_config_is_not_a_consistent_single_owner_case() {
    let (root, mut case) = fixture();
    case["configuration"][0]["sources"].as_array_mut().unwrap().push(json!({"name":"client override","evidence":"failure","pointer":"/data/client_timeout_ms"}));
    let report = run(root.path(), &case).unwrap();
    assert_eq!(report.status, "contradicted");
    assert!(has_finding(&report, "configuration_precedence_conflict"));
}

#[test]
fn refuted_hypothesis_cannot_justify_the_proposed_patch() {
    let (root, mut case) = fixture();
    case["hypotheses"][0]["experiments"][0]["outcome"] = json!("refutes");
    case["hypotheses"][0]["counterevidence"] = json!(["trial"]);
    let report = run(root.path(), &case).unwrap();
    assert_eq!(report.status, "contradicted");
    assert!(has_finding(&report, "hypothesis_refuted"));
}

#[test]
fn repeated_failed_attempts_require_reframing_before_another_patch() {
    let (root, mut case) = fixture();
    case["hypotheses"][0]["experiments"][0]["outcome"] = json!("inconclusive");
    case["hypotheses"][0]["experiments"][0]["observations"] = json!(["failure"]);
    case["attempts"] = json!([
        {"id":"retry-one","hypothesis":"discarded-canonical-setting","summary":"Increase retry count while retaining both timeout definitions.","observations":["failure"],"outcome":"failed"},
        {"id":"retry-two","hypothesis":"discarded-canonical-setting","summary":"Increase retry count while retaining both timeout definitions.","observations":["failure"],"outcome":"failed"}
    ]);
    let report = run(root.path(), &case).unwrap();
    assert_ne!(report.status, "consistent");
    assert!(has_finding(&report, "repeated_failed_attempts"));
}

#[test]
fn distinct_failed_experiments_are_not_mislabeled_as_identical_retries() {
    let (root, mut case) = fixture();
    case["hypotheses"][0]["experiments"][0]["outcome"] = json!("inconclusive");
    case["hypotheses"][0]["experiments"][0]["observations"] = json!(["failure"]);
    add_observation(
        root.path(),
        &mut case,
        "different-failure",
        json!({"stage":"client","symptom_persists":true,"intervention":"Remove retry override while preserving the independently configured timeout","client_timeout_ms":1000}),
    );
    case["attempts"] = json!([
        {"id":"probe-one","hypothesis":"discarded-canonical-setting","summary":"Increase retry count while retaining both timeout definitions.","observations":["failure"],"outcome":"failed"},
        {"id":"probe-two","hypothesis":"discarded-canonical-setting","summary":"Remove the retry override to distinguish retry behavior from timeout ownership.","observations":["different-failure"],"outcome":"failed"}
    ]);
    let report = run(root.path(), &case).unwrap();
    assert!(!has_finding(&report, "repeated_failed_attempts"));
    assert!(
        report
            .evidence
            .iter()
            .find(|item| item.id == "different-failure")
            .unwrap()
            .verified
    );
}

#[test]
fn absent_causal_edge_and_original_symptom_evidence_are_explicit_gaps() {
    let (root, mut case) = fixture();
    case["links"] = json!([]);
    let report = run(root.path(), &case).unwrap();
    assert_ne!(report.status, "consistent");
    assert!(has_finding(&report, "incomplete_trace"));
    let (root, mut case) = fixture();
    case["problem"]["failure_observations"] = json!([]);
    let report = run(root.path(), &case).unwrap();
    assert_ne!(report.status, "consistent");
    assert!(has_finding(&report, "missing_reproduction_evidence"));
}

#[test]
fn checked_observation_with_absent_original_symptom_does_not_prove_reproduction() {
    let (root, mut case) = fixture();
    mutate_observation(root.path(), &mut case, "failure", |value| {
        value["data"]["symptom_persists"] = json!(false)
    });
    let failure = case["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["id"] == "failure")
        .unwrap();
    failure["assertions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["pointer"] == "/data/symptom_persists")
        .unwrap()["expected"] = json!(false);
    let report = run(root.path(), &case).unwrap();
    assert!(
        report
            .evidence
            .iter()
            .find(|item| item.id == "failure")
            .unwrap()
            .verified
    );
    assert!(has_finding(&report, "missing_reproduction_evidence"));
    assert_ne!(report.status, "consistent");
}

#[test]
fn a_schema_file_pin_without_enforcement_observation_is_not_enforcement() {
    let (root, mut case) = fixture();
    let bytes = br#"{"type":"integer","minimum":0}"#;
    let relative = "examples/framing-case/timeout.schema.json";
    std::fs::write(root.path().join(relative), bytes).unwrap();
    case["evidence"].as_array_mut().unwrap().push(json!({"id":"timeout-schema","kind":"schema","path":relative,"sha256":hash(bytes),"assertions":[]}));
    case["stages"][0]["schemas"] = json!(["timeout-schema"]);
    case["stages"][0]["schema_not_applicable"] = Value::Null;
    let report = run(root.path(), &case).unwrap();
    assert_ne!(report.status, "consistent");
    assert!(has_finding(&report, "schema_enforcement_unverified"));
    assert!(
        report
            .evidence
            .iter()
            .any(|item| item.id == "timeout-schema" && item.verified)
    );
}

#[test]
fn schema_enforcement_needs_true_content_bound_to_the_actual_stage_and_schema() {
    let (root, mut case) = fixture();
    let bytes = br#"{"type":"integer","minimum":0}"#;
    let path = "examples/framing-case/timeout.schema.json";
    std::fs::write(root.path().join(path), bytes).unwrap();
    case["evidence"].as_array_mut().unwrap().push(json!({"id":"timeout-schema","kind":"schema","path":path,"sha256":hash(bytes),"assertions":[]}));
    case["stages"][0]["schemas"] = json!(["timeout-schema"]);
    case["stages"][0]["schema_not_applicable"] = Value::Null;
    case["stages"][0]["enforcement_observations"] = json!(["schema-check"]);
    add_observation(
        root.path(),
        &mut case,
        "schema-check",
        json!({"stage":"normalize","schema":"timeout-schema","enforced":false}),
    );
    let report = run(root.path(), &case).unwrap();
    assert!(
        report
            .evidence
            .iter()
            .find(|item| item.id == "schema-check")
            .unwrap()
            .verified
    );
    assert!(has_finding(&report, "schema_enforcement_unverified"));
    mutate_observation(root.path(), &mut case, "schema-check", |value| {
        value["data"]["enforced"] = json!(true)
    });
    let evidence = case["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["id"] == "schema-check")
        .unwrap();
    evidence["assertions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["pointer"] == "/data/enforced")
        .unwrap()["expected"] = json!(true);
    let report = run(root.path(), &case).unwrap();
    assert_eq!(
        report.status,
        "consistent",
        "{}",
        serde_json::to_string(&report).unwrap()
    );
}

#[test]
fn a_generic_reused_observation_cannot_manufacture_a_causal_edge() {
    let (root, mut case) = fixture();
    mutate_observation(root.path(), &mut case, "failure", |value| {
        value["data"]["from"] = json!("unrelated-producer")
    });
    let failure = case["evidence"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["id"] == "failure")
        .unwrap();
    failure["assertions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["pointer"] == "/data/from")
        .unwrap()["expected"] = json!("unrelated-producer");
    case["links"][0]["observations"] = json!(["failure"]);
    let report = run(root.path(), &case).unwrap();
    assert_ne!(report.status, "consistent");
    assert!(has_finding(&report, "unobserved_link"));
    assert!(
        report
            .evidence
            .iter()
            .find(|item| item.id == "failure")
            .unwrap()
            .verified
    );
}

#[test]
fn missing_evidence_is_a_gap_but_dangling_ids_and_unknown_fields_are_errors() {
    let (root, case) = fixture();
    std::fs::remove_file(root.path().join("examples/framing-case/failure.json")).unwrap();
    let report = run(root.path(), &case).unwrap();
    assert_ne!(report.status, "consistent");
    assert!(
        report
            .evidence
            .iter()
            .any(|item| item.id == "failure" && item.state == "missing")
    );
    let (root, mut case) = fixture();
    case["links"][0]["to"] = json!("absent-stage");
    assert!(run(root.path(), &case).is_err());
    let (root, mut case) = fixture();
    case["problem"]["assume_cause_proven"] = json!(true);
    assert!(
        run(root.path(), &case)
            .unwrap_err()
            .contains("unknown field")
    );
}

#[test]
fn path_escape_and_dangling_evidence_never_produce_a_consistent_report() {
    let (root, mut case) = fixture();
    case["evidence"][0]["path"] = json!("../outside.rs");
    assert!(run(root.path(), &case).unwrap_err().contains("path"));
    let (root, mut case) = fixture();
    case["hypotheses"][0]["support"] = json!(["absent-evidence"]);
    assert!(run(root.path(), &case).is_err());
}

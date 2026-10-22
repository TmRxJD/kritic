use kritic::scan::parse_cargo_audit;
use serde_json::{Value, json};

fn audit(vector: Value) -> String {
    json!({"vulnerabilities": {"list": [{
        "advisory": {"id": "RUSTSEC-TEST", "title": "Advisory", "cvss": vector},
        "package": {"name": "fixture", "version": "1.0.0"}
    }]}})
    .to_string()
}

#[test]
fn scores_rustsec_vectors_instead_of_treating_all_vectors_as_high() {
    // Public RustSec advisories 2026-0346 through 0349: the original bug rated all four high.
    for (vector, severity, score) in [
        (
            "CVSS:3.1/AV:L/AC:H/PR:H/UI:R/S:U/C:H/I:N/A:N",
            "medium",
            "4.0",
        ),
        ("CVSS:3.1/AV:L/AC:L/PR:H/UI:N/S:U/C:L/I:N/A:N", "low", "2.3"),
        ("CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:N/I:N/A:L", "low", "3.7"),
        ("CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:L/I:N/A:N", "low", "3.7"),
        (
            "CVSS:3.0/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H",
            "critical",
            "9.8",
        ),
        (
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:H/I:H/A:H",
            "high",
            "8.8",
        ),
        (
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N",
            "info",
            "0.0",
        ),
        (
            "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H",
            "critical",
            "10.0",
        ),
    ] {
        let findings = parse_cargo_audit(&audit(json!(vector))).unwrap();
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.severity, severity, "{vector}");
        assert!(finding.message.ends_with(&format!("(CVSS {score})")));
        assert_eq!(finding.rule, "RUSTSEC-TEST");
        assert_eq!(finding.file, "Cargo.lock");
    }
}

#[test]
fn unscored_advisories_keep_explicit_policy_severity() {
    let findings = parse_cargo_audit(&audit(Value::Null)).unwrap();
    assert_eq!(findings[0].severity, "medium");
    assert!(
        findings[0]
            .message
            .contains("CVSS unavailable; policy severity")
    );
}

#[test]
fn malformed_and_unsupported_vectors_fail_instead_of_becoming_high() {
    for vector in [
        "",
        "CVSS:3.1/AV:N",
        "CVSS:9.0/AV:N",
        "not a vector",
        "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H/AV:L",
    ] {
        let error = parse_cargo_audit(&audit(json!(vector))).unwrap_err();
        assert!(error.starts_with("cargo-audit CVSS for RUSTSEC-TEST:"));
    }
    assert!(
        parse_cargo_audit("not JSON")
            .unwrap_err()
            .starts_with("cargo-audit JSON:")
    );
    assert!(
        parse_cargo_audit(r#"{"vulnerabilities":{"list":[]}}"#)
            .unwrap()
            .is_empty()
    );
}

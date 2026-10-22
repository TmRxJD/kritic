//! Regression coverage for ACS content identities without suppressing real credentials.
use kritic::scan;

#[test]
fn content_identity_exception_is_exact_field_and_rule_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let path = scan::gitleaks_config(dir.path()).unwrap();
    let config: toml::Table = std::fs::read_to_string(path).unwrap().parse().unwrap();
    assert_eq!(config["extend"]["useDefault"].as_bool(), Some(true));
    let rules = config["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["id"].as_str(), Some("generic-api-key"));
    let allow = &rules[0]["allowlists"][0];
    assert_eq!(allow["regexTarget"].as_str(), Some("match"));
    assert!(allow.get("paths").is_none());
    let pattern = regex::Regex::new(allow["regexes"][0].as_str().unwrap()).unwrap();
    let hash = synthetic_hash();
    assert!(pattern.is_match(&format!("canonical_key\":\"acs-decision-{hash}\"")));
    assert!(pattern.is_match(&format!("\"canonical_key\": \"acs-decision-{hash}\"")));
    for candidate in [
        format!("api_key\":\"acs-decision-{hash}\""),
        format!("canonical_key\":\"{hash}\""),
        format!("canonical_key\":\"acs-decision-{hash}0\""),
        format!("canonical_key\":\"acs-decision-{hash}\",\"api_key\":\"other-secret\""),
    ] {
        assert!(
            !pattern.is_match(&candidate),
            "exception widened to {candidate}"
        );
    }
}

use sha2::Digest;

fn synthetic_hash() -> String {
    sha2::Sha256::digest(b"ACS synthetic decision regression")
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
#[ignore = "requires KRITIC_GITLEAKS_BIN pointing at the real scanner"]
fn real_scanner_keeps_credentials_next_to_content_id_on_the_same_line() {
    let executable = std::env::var_os("KRITIC_GITLEAKS_BIN").expect("scanner path required");
    let temp = tempfile::tempdir().unwrap();
    let conf_dir = temp.path().join("config");
    let input = temp.path().join("input");
    std::fs::create_dir(&conf_dir).unwrap();
    std::fs::create_dir(&input).unwrap();
    let config = scan::gitleaks_config(&conf_dir).unwrap();
    let hash = synthetic_hash();
    // Not a credential: the fake value is a deterministic, namespaced content ID.
    let namespaced = format!("acs-decision-{hash}");
    std::fs::write(
        input.join("identity.json"),
        format!("{{\"canonical_key\":\"{namespaced}\"}}\n"),
    )
    .unwrap();
    // A field named api_key must remain a finding even if it contains the same bytes.
    std::fs::write(
        input.join("credential.json"),
        format!("{{\"api_key\":\"{namespaced}\"}}\n"),
    )
    .unwrap();
    std::fs::write(
        input.join("mixed.json"),
        format!("{{\"canonical_key\":\"{namespaced}\",\"api_key\":\"{namespaced}\"}}\n"),
    )
    .unwrap();
    let output = std::process::Command::new(executable)
        .args(["dir", "--config"])
        .arg(config)
        .args([
            "--report-format",
            "json",
            "--report-path",
            "-",
            "--no-banner",
            "--redact",
            "--exit-code",
            "0",
        ])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let findings =
        scan::parse_gitleaks(&input, &String::from_utf8(output.stdout).unwrap()).unwrap();
    assert!(
        !findings.iter().any(|f| f.file.ends_with("identity.json")),
        "{findings:?}"
    );
    for expected in ["credential.json", "mixed.json"] {
        assert!(
            findings
                .iter()
                .any(|f| f.file.ends_with(expected) && f.rule == "generic-api-key"),
            "{findings:?}"
        );
    }
}

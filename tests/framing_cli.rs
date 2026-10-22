//! Small tool surface: a case path, never inline debugging state or executable commands.
use clap::Parser;
use kritic::cli::Cli;

#[test]
fn framing_cli_accepts_case_or_schema_but_not_both() {
    assert!(Cli::try_parse_from(["kritic", "framing", "--schema"]).is_ok());
    assert!(
        Cli::try_parse_from([
            "kritic",
            "framing",
            "--base-dir",
            ".",
            "--case",
            ".kritic/debug-case.json"
        ])
        .is_ok()
    );
    assert!(Cli::try_parse_from(["kritic", "framing", "--schema", "--case", "case.json"]).is_err());
    assert!(Cli::try_parse_from(["kritic", "framing", "--run-commands"]).is_err());
}

#[test]
fn framing_request_rejects_inline_commands_and_requires_base() {
    assert!(
        serde_json::from_value::<kritic::framing::Request>(
            serde_json::json!({"base_dir":".","commands":["touch marker"]})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<kritic::framing::Request>(
            serde_json::json!({"case_path":"case.json"})
        )
        .is_err()
    );
    let request: kritic::framing::Request =
        serde_json::from_value(serde_json::json!({"base_dir":"."})).unwrap();
    assert!(request.case_path.is_none());
}

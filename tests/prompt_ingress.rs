use clap::Parser;
use kritic::{cli::Cli, config::Config};

#[tokio::test]
async fn deterministic_intake_preserves_source_and_never_grants_replacement() {
    let mut cfg = Config::load();
    cfg.config_errors.clear();
    cfg.prompt_backend = "off".into();
    let original = "Never delete my unfinished work. Use the current fixture only.";
    let report = kritic::prompt::review(&cfg, original, false).await.unwrap();
    assert_eq!(report["original"], original);
    assert_eq!(report["replacement_allowed"], false);
    assert_eq!(report["status"], "keep");
}

#[test]
fn login_cannot_be_triggered_from_the_hook_command() {
    assert!(Cli::try_parse_from(["kritic", "prompt", "--hook", "codex"]).is_ok());
    assert!(Cli::try_parse_from(["kritic", "prompt", "--login", "codex"]).is_ok());
    assert!(
        Cli::try_parse_from(["kritic", "prompt", "--hook", "codex", "--login", "codex"]).is_err()
    );
}

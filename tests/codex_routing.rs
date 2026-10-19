use kritic::tools::codex_model;

#[test]
fn only_codex_models_execute_and_uncertainty_stays_on_sol() {
    assert_eq!(codex_model(Some("luna"), Some(0.95)), "gpt-6-luna");
    assert_eq!(codex_model(Some("astra"), Some(0.90)), "gpt-6-astra");
    for (tier, confidence) in [
        (Some("astra"), Some(0.3)),
        (Some("jev"), Some(0.99)),
        (None, None),
        (Some("luna"), Some(f64::NAN)),
        (Some("astra"), Some(1.1)),
    ] {
        assert_eq!(codex_model(tier, confidence), "gpt-6.1-sol");
    }
    assert!(kritic::spec::lint_builtin().is_empty());
}

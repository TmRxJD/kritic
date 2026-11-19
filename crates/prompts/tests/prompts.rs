use kritic_prompts::{evaluate, hook_context};
use serde_json::json;

fn proposal(original: &str, brief: &str) -> serde_json::Value {
    json!({"action":"rewrite","brief":brief,"intent_map":[{"source_quote":original,"brief_quote":brief}],"questions":[],"reasons":["Remove repetition"]})
}

#[test]
fn useful_brief_is_advisory_and_exact_original_is_retained() {
    let original = "Please update src/main.rs and run cargo test. Update src/main.rs, please.";
    let report = evaluate(
        original,
        Some(proposal(original, "Update src/main.rs and run cargo test.")),
    )
    .unwrap();
    assert_eq!(report["status"], "suggestion");
    assert_eq!(report["original"], original);
    assert_eq!(report["replacement_allowed"], false);
    assert!(
        hook_context(&report)
            .unwrap()
            .contains("original user message controls")
    );
}

#[test]
fn lost_paths_versions_and_prohibitions_are_rejected() {
    for original in [
        "Please update src/main.rs carefully and avoid unrelated changes.",
        "Please use version 2.4.1 and carefully check the behavior.",
        "Please do not deploy the change, just test it thoroughly.",
    ] {
        let report = evaluate(original, Some(proposal(original, "Update and test."))).unwrap();
        assert_eq!(report["status"], "rejected_proposal");
        assert!(hook_context(&report).is_none());
    }
}

#[test]
fn invented_commands_and_uncited_mapping_are_rejected() {
    let original = "Could you improve the parser carefully and test all its inputs?";
    assert_eq!(
        evaluate(original, Some(proposal(original, "Run `rm -rf src`"))).unwrap()["status"],
        "rejected_proposal"
    );
    let mut bad = proposal(original, "Improve and test the parser.");
    bad["intent_map"][0]["source_quote"] = json!("made up requirement");
    assert_eq!(
        evaluate(original, Some(bad)).unwrap()["status"],
        "rejected_proposal"
    );
}

#[test]
fn ambiguity_asks_questions_without_rewriting_original() {
    let original = "Make it better";
    let report=evaluate(original,Some(json!({"action":"clarify","brief":original,"intent_map":[{"source_quote":original,"brief_quote":original}],"questions":["Which observable result should improve?"],"reasons":[]}))).unwrap();
    assert_eq!(report["status"], "clarification");
    assert_eq!(report["original"], original);
}

#[test]
fn empty_oversized_and_unknown_fields_do_not_pass() {
    assert!(evaluate(" ", None).is_err());
    assert!(evaluate(&"x".repeat(65537), None).is_err());
    let mut bad = proposal("hello there", "hello");
    bad["extra"] = json!(true);
    assert!(evaluate("hello there", Some(bad)).is_err());
}

#[test]
fn request_schema_and_runtime_both_require_grounded_model_assessments() {
    assert_eq!(
        kritic_prompts::schema()["properties"]["intent_map"]["minItems"],
        1
    );
    for action in ["keep", "rewrite", "clarify"] {
        let report=evaluate("Please list 3 ideas. Please list 3 ideas.",Some(json!({"action":action,"brief":"List 3 ideas.","intent_map":[],"questions":[],"reasons":[]}))).unwrap();
        assert_eq!(report["status"], "rejected_proposal");
        assert!(hook_context(&report).is_none());
    }
}

#[test]
fn fenced_code_is_protected_and_clear_prompts_are_not_penalized() {
    let original = "Fix this exactly: ```rust\nlet x = 2;\n``` please fix it.";
    assert_eq!(
        evaluate(original, Some(proposal(original, "Fix the code."))).unwrap()["status"],
        "rejected_proposal"
    );
    let report = evaluate("Run the tests.", None).unwrap();
    assert_eq!(report["status"], "keep");
    assert!(report["findings"].as_array().unwrap().is_empty());
}

#[test]
fn moving_not_elsewhere_does_not_preserve_a_prohibition() {
    let original = "Please do not deploy. Carefully test the changes and test them again.";
    let brief = "Deploy. Do not skip tests.";
    assert_eq!(
        evaluate(original, Some(proposal(original, brief))).unwrap()["status"],
        "rejected_proposal"
    );
}

#[test]
fn protected_paragraphs_do_not_split_filenames_urls_decimals_or_line_wraps() {
    for (original, brief) in [
        (
            "Do not change src/foo.rs permissions. Please check the parser carefully.",
            "Do not change src/foo.rs contents. Check parser.",
        ),
        (
            "Do not send data to https://example.com/upload before approval. Check everything carefully.",
            "Do not send data to https://example.com/upload after approval.",
        ),
        (
            "Do not exceed 2.5 seconds per request. Please check everything carefully.",
            "Do not exceed 2.5 minutes per request.",
        ),
        (
            "Do not change\nthe permissions. Please inspect this carefully.",
            "Do not change\nthe contents.",
        ),
    ] {
        assert_eq!(
            evaluate(original, Some(proposal(original, brief))).unwrap()["status"],
            "rejected_proposal"
        );
    }
}

#[test]
fn rewrite_can_preserve_protected_paragraph_and_shorten_unprotected_repetition() {
    let original = "Do not change src/foo.rs permissions.\n\nCheck the parser carefully. Check the parser carefully.";
    let brief = "Do not change src/foo.rs permissions.\n\nCheck the parser carefully.";
    assert_eq!(
        evaluate(original, Some(proposal(original, brief))).unwrap()["status"],
        "suggestion"
    );
}

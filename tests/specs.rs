//! Every built-in decision spec parses and passes lint; the linter rejects each rule it claims to enforce.

use kritic::spec::{self, Ask, DecisionSpec, advisory, band, code, noul, text, yes_above, Vocabulary};

#[test]
fn builtin_specs_are_valid() {
    let errors = spec::lint_builtin();
    assert!(errors.is_empty(), "invalid built-in specs:\n  {}", errors.join("\n  "));
    assert_eq!(spec::builtin().len(), 11);
}

fn base() -> DecisionSpec {
    DecisionSpec {
        name: "t".into(),
        purpose: "test spec".into(),
        fields: vec![code("change", 100, "A unified diff hunk for testing.")],
        questions: vec![noul("defect", "Does `change` add a defect?", "an added line misbehaves", "the added lines behave", advisory("not calibrated in this test"))],
        calibrated_for: vec!["kev-4b".into()],
    }
}

fn errs(s: &DecisionSpec) -> String {
    s.lint().join(" | ")
}

#[test]
fn a_well_formed_spec_lints_clean() {
    assert!(base().lint().is_empty(), "{}", errs(&base()));
}

#[test]
fn lint_rejects_questions_that_name_no_field_or_an_unknown_one() {
    let mut s = base();
    s.questions[0].ask = Ask::Noul { instructions: "Is this code buggy?".into(), yes: "a line misbehaves".into(), no: "no line misbehaves".into() };
    assert!(errs(&s).contains("names no state field"), "{}", errs(&s));
    s.questions[0].ask = Ask::Noul { instructions: "Does `patch` add a defect?".into(), yes: "a line misbehaves".into(), no: "no line misbehaves".into() };
    assert!(errs(&s).contains("not a declared field"), "{}", errs(&s));
}

#[test]
fn lint_rejects_compound_yes_no_questions_and_undefined_answers() {
    let mut s = base();
    s.questions[0].ask = Ask::Noul { instructions: "Does `change` add a bug or a security hole?".into(), yes: "a line misbehaves".into(), no: "".into() };
    let e = errs(&s);
    assert!(e.contains("one condition") && e.contains("must be defined"), "{e}");
}

#[test]
fn lint_rejects_gates_without_dated_calibration_evidence() {
    let mut s = base();
    s.questions[0].gate = yes_above(0.5, "defect", "seems fine");
    assert!(errs(&s).contains("calibration evidence"), "{}", errs(&s));
    s.questions[0].gate = yes_above(0.5, "defect", "2026-09-27: 40 labelled hunks, clean mean 0.12 vs buggy mean 0.81");
    assert!(s.lint().is_empty(), "{}", errs(&s));
}

#[test]
fn lint_rejects_unread_fields_and_too_many_reads() {
    let mut s = base();
    s.fields.push(text("task", 100, "The request, for testing."));
    assert!(errs(&s).contains("no question reads it"), "{}", errs(&s));
    for n in ["a", "b"] {
        s.fields.push(text(n, 100, "Another field for testing."));
    }
    s.questions[0].ask = Ask::Noul { instructions: "Do `change`, `task`, `a` and `b` agree?".into(), yes: "they all agree".into(), no: "some disagree".into() };
    assert!(errs(&s).contains("at most 2"), "{}", errs(&s));
}

#[test]
fn lint_requires_band_questions_to_use_the_vocabulary() {
    let v = Vocabulary::new("range", &[("TYPICAL", "within ranked maps"), ("BEYOND", "outside every ranked map")]);
    let mut s = base();
    s.fields = vec![band("parity", &v, "Parity break rate judged by code.")];
    s.questions[0].ask = Ask::Noul { instructions: "Is `parity` fine?".into(), yes: "it is fine here".into(), no: "it is not fine".into() };
    assert!(errs(&s).contains("uses none of its terms"), "{}", errs(&s));
    s.questions[0].ask = Ask::Noul { instructions: "Is `parity` TYPICAL?".into(), yes: "it reads TYPICAL".into(), no: "it reads BEYOND".into() };
    assert!(s.lint().is_empty(), "{}", errs(&s));
}

#[test]
fn state_requires_exactly_the_declared_fields_within_limits() {
    let s = base();
    assert!(s.state(&[("change", "+ let x = 1;".into())]).is_ok());
    assert!(s.state(&[]).unwrap_err().contains("missing"));
    assert!(s.state(&[("change", "x".into()), ("extra", "y".into())]).unwrap_err().contains("not a declared field"));
    assert!(s.state(&[("change", "x".repeat(101))]).unwrap_err().contains("chunk it"));
    let v = Vocabulary::new("range", &[("TYPICAL", "within ranked maps"), ("BEYOND", "outside every ranked map")]);
    let b = DecisionSpec { fields: vec![band("parity", &v, "Parity break rate judged by code.")], ..base() };
    assert!(b.state(&[("parity", "BEYOND: 0.79 per note".into())]).is_ok());
    assert!(b.state(&[("parity", "0.79".into())]).unwrap_err().contains("term"));
}

#[test]
fn specs_load_from_json_with_every_error_at_once() {
    let bad = r#"{"name":"x","purpose":"p","fields":[],"questions":[],"extra":1}"#;
    assert!(spec::load(bad).unwrap_err()[0].contains("schema"));
    let empty = r#"{"name":"x","purpose":"a purpose","fields":[],"questions":[]}"#;
    let e = spec::load(empty).unwrap_err();
    assert!(e.iter().any(|x| x.contains("no fields")) && e.iter().any(|x| x.contains("no questions")), "{e:?}");
}

#[test]
fn gates_tighten_with_the_number_of_units() {
    // 29 clean hunks at a flat 5% per-hunk threshold: 1 - 0.95^29 = 77% of clean changes rejected.
    let mut s = base();
    s.questions[0].gate = spec::Gate::Rule {
        measure: spec::Measure::Yes,
        option: None,
        fails_when: spec::Cmp::Above,
        threshold: 0.38,
        reason: "defect".into(),
        calibration: "2026-09-27: 130 good vs 60 bad hunks, AUC 0.92, 5% false rejects".into(),
        good_tail: vec![(0.05, 0.38), (0.02, 0.45), (0.01, 0.50), (0.005, 0.53), (0.0, 0.53)],
    };
    assert!(s.lint().is_empty(), "{}", errs(&s));
    assert_eq!(s.threshold_for("defect", 1), Some(0.38));
    assert_eq!(s.threshold_for("defect", 2), Some(0.45)); // 2.5% per unit -> the 2% entry
    assert_eq!(s.threshold_for("defect", 5), Some(0.50)); // 1%
    assert_eq!(s.threshold_for("defect", 29), Some(0.53)); // beyond the data: strictest seen
    assert!(s.check_units("defect", 0.43, 1).is_some());
    assert!(s.check_units("defect", 0.43, 29).is_none());
    // the tail must agree with the threshold and tighten as the rate falls
    if let spec::Gate::Rule { good_tail, .. } = &mut s.questions[0].gate {
        good_tail[2].1 = 0.40;
    }
    assert!(errs(&s).contains("stricter"), "{}", errs(&s));
}

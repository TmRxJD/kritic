use base64::Engine;
use kritic::visual_quality::{self, DIMENSIONS, ProvisionalStatus, QualityTarget};
use serde_json::{Value, json};

fn craft(score: f64) -> Value {
    let mut dimensions = serde_json::Map::new();
    for name in DIMENSIONS {
        dimensions.insert(name.into(), json!({"score":score,"coverage":"visible",
            "evidence":[{"element":"Settings panel labels","observation":"Type weights and spacing visibly distinguish headings from field labels."}],
            "improvement":"Align the settings labels to one consistent baseline and spacing interval."}));
    }
    json!({"craft": dimensions})
}

#[test]
fn profile_floors_cannot_be_lowered() {
    for (target, floor) in [
        (QualityTarget::Functional, 6.0),
        (QualityTarget::Professional, 8.0),
        (QualityTarget::Showcase, 9.0),
    ] {
        let result = visual_quality::parse_quality(&craft(9.0), target, 2.0).unwrap();
        assert_eq!(result.minimum, floor);
        assert_eq!(result.provisional_status, ProvisionalStatus::NeedsReview);
    }
    let result =
        visual_quality::parse_quality(&craft(9.0), QualityTarget::Professional, 9.5).unwrap();
    assert_eq!(result.minimum, 9.5);
    assert_eq!(result.provisional_status, ProvisionalStatus::BelowTarget);
}

#[test]
fn weak_dimension_cannot_hide_behind_high_scores() {
    let mut value = craft(10.0);
    value["craft"]["typography"]["score"] = json!(7.0);
    let result = visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).unwrap();
    assert_eq!(result.weakest_dimension.as_deref(), Some("typography"));
    assert_eq!(result.weakest_score, Some(7.0));
    assert_eq!(result.provisional_status, ProvisionalStatus::BelowTarget);
    assert!(result.evidence_unverified);
}

#[test]
fn absent_and_partial_evidence_never_establish_target() {
    for coverage in ["partial", "not_visible"] {
        let mut value = craft(10.0);
        value["craft"]["asset_coherence"]["coverage"] = json!(coverage);
        let result =
            visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).unwrap();
        assert_eq!(
            result.provisional_status,
            ProvisionalStatus::InsufficientEvidence
        );
    }
    let mut value = craft(10.0);
    value["craft"]["typography"]["evidence"] = json!([]);
    assert_eq!(
        visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0)
            .unwrap()
            .provisional_status,
        ProvisionalStatus::InsufficientEvidence
    );
}

#[test]
fn legacy_payload_remains_insufficient_for_craft() {
    let value = json!({"layout_clarity":10,"spacing_alignment":10,"visual_hierarchy":10,
        "color_contrast":10,"accessibility":10,"spacing_issues":[],"contrast_issues":[],"accessibility_issues":[]});
    assert!(kritic::vision::parse(&value).is_ok());
    let result = visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).unwrap();
    assert_eq!(
        result.provisional_status,
        ProvisionalStatus::InsufficientEvidence
    );
    assert!(result.findings.is_empty());
    let mut legacy = kritic::vision::VisualResult::default();
    let (scores, observations) = kritic::vision::parse(&value).unwrap();
    kritic::vision::judge(&mut legacy, scores, observations, 6.0);
    assert_eq!(legacy.approval, None);
    assert_eq!(legacy.legacy_scores_meet_threshold, Some(true));
}

#[test]
fn aaa_alias_and_default_are_explicit() {
    assert_eq!(QualityTarget::default(), QualityTarget::Professional);
    assert_eq!(
        QualityTarget::parse("AAA").unwrap(),
        QualityTarget::Showcase
    );
    assert_eq!(
        serde_json::from_value::<QualityTarget>(json!("aaa")).unwrap(),
        QualityTarget::Showcase
    );
    assert!(QualityTarget::parse("anything").is_err());
}

#[test]
fn invalid_scores_bounds_and_missing_improvements_fail_closed() {
    for score in [-1.0, 11.0, f64::NAN, f64::INFINITY] {
        assert!(
            visual_quality::parse_quality(&craft(score), QualityTarget::Professional, 6.0).is_err()
        );
    }
    assert!(
        visual_quality::parse_quality(&craft(8.0), QualityTarget::Professional, f64::NAN).is_err()
    );
    let mut value = craft(7.0);
    value["craft"]["typography"]["improvement"] = json!("");
    assert!(visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).is_err());
    let mut value = craft(9.0);
    value["craft"]["typography"]["evidence"][0]["element"] = json!("x".repeat(161));
    assert!(visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).is_err());
    let mut value = craft(9.0);
    value["craft"].as_object_mut().unwrap().remove("typography");
    assert!(visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).is_err());
}

#[test]
fn evidence_is_limited_to_two_concrete_observations() {
    let mut value = craft(9.0);
    let evidence = value["craft"]["typography"]["evidence"][0].clone();
    value["craft"]["typography"]["evidence"] = json!([evidence, evidence, evidence]);
    assert!(visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).is_err());
}

#[test]
fn perfect_scores_with_generic_praise_do_not_establish_craft() {
    let mut value = craft(10.0);
    value["craft"]["typography"]["evidence"] =
        json!([{"element":" UI ", "observation":"LOOKS GOOD"}]);
    assert!(visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).is_err());
    value["craft"]["typography"]["evidence"][0]["element"] = json!("Toolbar primary action label");
    assert!(visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).is_err());
    value["craft"]["typography"]["evidence"] = json!([{"element":"Toolbar primary action label", "observation":"The semibold Save label has a distinct weight from adjacent regular navigation labels."}]);
    assert_eq!(
        visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0)
            .unwrap()
            .provisional_status,
        ProvisionalStatus::NeedsReview
    );
    value["craft"]["typography"]["score"] = json!(7.0);
    value["craft"]["typography"]["improvement"] = json!(" Modernize ");
    assert!(visual_quality::parse_quality(&value, QualityTarget::Professional, 6.0).is_err());
}

#[test]
fn reference_payload_keeps_subject_first_and_records_provenance() {
    let directory = tempfile::tempdir().unwrap();
    let subject = directory.path().join("subject.png");
    let reference = directory.path().join("comparison.png");
    // Signature-bearing byte fixtures test payload binding only, never decoded image validity.
    let subject_bytes = b"\x89PNG\r\n\x1a\nsubject-fixture";
    let reference_bytes = b"\x89PNG\r\n\x1a\nreference-fixture";
    std::fs::write(&subject, subject_bytes).unwrap();
    std::fs::write(&reference, reference_bytes).unwrap();
    let payload = kritic::vision::prepare_visual_payload(
        &subject,
        &[reference.clone()],
        "Settings app",
        QualityTarget::Professional,
        6.0,
    )
    .unwrap();
    let decode = |text: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .unwrap()
    };
    assert_eq!(decode(&payload.images[0]), subject_bytes);
    assert_eq!(decode(&payload.images[1]), reference_bytes);
    assert_eq!(payload.subject.role, "subject");
    assert_eq!(payload.references[0].role, "reference_1");
    assert_eq!(payload.references[0].path, reference.display().to_string());
    assert_eq!(payload.references[0].bytes, reference_bytes.len() as u64);
    assert_eq!(payload.subject.sha256.len(), 64);
    assert_ne!(payload.subject.sha256, payload.references[0].sha256);
    assert!(
        payload
            .prompt
            .contains("Every craft evidence element and observation must refer to image 1 SUBJECT")
    );
    let solo =
        kritic::vision::prepare_visual_payload(&subject, &[], "", QualityTarget::Professional, 6.0)
            .unwrap();
    assert_eq!(solo.images.len(), 1);
    assert!(solo.references.is_empty());
    assert!(solo.prompt.contains("No comparison images are attached"));
}

#[test]
fn reference_payload_rejects_excess_count_size_and_unsupported_content() {
    let directory = tempfile::tempdir().unwrap();
    let subject = directory.path().join("subject.png");
    std::fs::write(&subject, b"\x89PNG\r\n\x1a\nfixture").unwrap();
    let prepare = |references: &[std::path::PathBuf]| {
        kritic::vision::prepare_visual_payload(
            &subject,
            references,
            "",
            QualityTarget::Professional,
            6.0,
        )
    };
    assert!(
        prepare(&[subject.clone(), subject.clone(), subject.clone()])
            .unwrap_err()
            .contains("at most two")
    );
    let large = directory.path().join("large.png");
    std::fs::File::create(&large)
        .unwrap()
        .set_len(kritic::vision::MAX_VISUAL_IMAGE_BYTES as u64 + 1)
        .unwrap();
    assert!(prepare(&[large]).unwrap_err().contains("exceeds 5 MiB"));
    let text = directory.path().join("not-an-image.png");
    std::fs::write(&text, "reference URL or text cannot be attached as pixels").unwrap();
    assert!(prepare(&[text]).unwrap_err().contains("must be PNG"));
    let gif = directory.path().join("unsupported.gif");
    std::fs::write(&gif, b"GIF89afixture").unwrap();
    assert!(prepare(&[gif]).unwrap_err().contains("must be PNG"));
    assert!(
        kritic::vision::prepare_visual_payload(
            &subject,
            &[],
            "",
            QualityTarget::Professional,
            f64::NAN
        )
        .unwrap_err()
        .contains("finite")
    );
}

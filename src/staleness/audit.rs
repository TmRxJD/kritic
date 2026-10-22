use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{Finding, Manifest, Observed, Report, files};

pub(super) fn finding(
    report: &mut Report,
    code: &str,
    path: &str,
    detail: impl Into<String>,
    action: &str,
) {
    report.findings.push(Finding {
        code: code.into(),
        path: path.into(),
        detail: detail.into(),
        action: action.into(),
    });
}

pub(super) fn fixture_bytes(bytes: &[u8], pointer: &str) -> Result<Vec<u8>, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("Canonical fixture source must be JSON: {e}"))?;
    let selected = value
        .pointer(pointer)
        .ok_or("Canonical JSON Pointer missing/invalid")?;
    let mut result = serde_json::to_vec_pretty(selected).map_err(|e| e.to_string())?;
    result.push(b'\n');
    if result.len() > files::MAX_FILE {
        return Err("Generated fixture exceeds 1 MiB".into());
    }
    Ok(result)
}

pub(super) fn check(base: &Path, manifest_path: &str, m: &Manifest, report: &mut Report) {
    let pattern = match regex::Regex::new(&m.test_pattern) {
        Ok(p) => p,
        Err(_) => return,
    };
    let discovered = match files::discover(base, &m.test_roots, &pattern) {
        Ok(found) => found,
        Err(e) => {
            finding(
                report,
                "inventory_incomplete",
                manifest_path,
                e,
                "Repair/narrow the explicit test corpus; do not claim all tests were reviewed.",
            );
            return;
        }
    };
    report.discovered_tests = discovered.len();
    if discovered.is_empty() {
        finding(
            report,
            "empty_inventory",
            manifest_path,
            "No test files matched the declared corpus",
            "Correct roots/pattern; empty discovery cannot establish freshness.",
        );
    }
    let found: BTreeSet<_> = discovered.iter().cloned().collect();
    let mut budget = files::MAX_TOTAL;
    let mut authorities = BTreeMap::new();
    for authority in &m.authorities {
        match files::read(base, &authority.path, &mut budget) {
            Ok(bytes) => {
                report.observed.push(Observed {
                    path: authority.path.clone(),
                    sha256: crate::contract::hash(&bytes),
                });
                authorities.insert(authority.id.clone(), bytes);
                if !m
                    .tests
                    .iter()
                    .any(|t| t.reviewed_inputs.contains_key(&authority.id))
                {
                    finding(
                        report,
                        "unused_authority",
                        &authority.path,
                        "No mapped test depends on this declared authority",
                        "Map real consumers or remove an incorrectly declared domain after review.",
                    );
                }
            }
            Err(e) => finding(
                report,
                "authority_unavailable",
                &authority.path,
                e,
                "Restore the canonical source; do not substitute a copied expectation.",
            ),
        }
    }
    for path in &discovered {
        if let Some(exclusion) = m.exclusions.iter().find(|e| &e.path == path) {
            report.excluded_tests += 1;
            finding(
                report,
                "excluded_test_unverified",
                path,
                format!("Excluded from review: {}", exclusion.reason),
                "Review this exclusion independently; it is not current-test evidence.",
            );
        } else if !m.tests.iter().any(|t| &t.path == path) {
            finding(
                report,
                "unmapped_test",
                path,
                "Discovered test has no canonical dependency/review binding",
                "Identify its actual contract and independent expected behavior, then add a reviewed binding.",
            );
        }
    }
    for exclusion in &m.exclusions {
        if !found.contains(&exclusion.path) {
            finding(
                report,
                "obsolete_exclusion",
                &exclusion.path,
                "Exclusion is outside the current discovered corpus",
                "Remove or correct the stale exclusion; do not widen the corpus blindly.",
            );
        }
    }
    for test in &m.tests {
        if !found.contains(&test.path) {
            finding(
                report,
                "test_not_discovered",
                &test.path,
                "Mapped test is missing or outside the declared corpus",
                "Restore the test or correct roots/pattern/mapping after reviewing its replacement.",
            );
            continue;
        }
        report.mapped_tests += 1;
        match files::read(base, &test.path, &mut budget) {
            Ok(bytes) => {
                let actual = crate::contract::hash(&bytes);
                if actual != test.reviewed_test_sha256 {
                    finding(
                        report,
                        "test_changed",
                        &test.path,
                        "Test bytes differ from the reviewed version; this is a review lead, not proof the test is wrong",
                        "Review assertions against intended behavior and meaningful execution before updating the review hash.",
                    );
                }
                let text = String::from_utf8_lossy(&bytes);
                if ["#[ignore", "pytest.mark.skip", ".skip(", ".todo(", "xfail"]
                    .iter()
                    .any(|marker| text.contains(marker))
                {
                    report.review_notes.push(format!("{} contains a skip/xfail/todo marker; text is a lead, not parsed execution coverage", test.path));
                }
                report.observed.push(Observed {
                    path: test.path.clone(),
                    sha256: actual,
                });
            }
            Err(e) => finding(
                report,
                "test_unavailable",
                &test.path,
                e,
                "Restore/read the actual test; do not fabricate a passing review hash.",
            ),
        }
        for (id, reviewed) in &test.reviewed_inputs {
            if let Some(bytes) = authorities.get(id) {
                let actual = crate::contract::hash(bytes);
                if &actual != reviewed {
                    finding(
                        report,
                        "authority_changed",
                        &test.path,
                        format!(
                            "Canonical input {id} changed: reviewed {reviewed}, current {actual}"
                        ),
                        "Trace the intended behavior change; distinguish an obsolete assertion from a real regression, run the test, then explicitly review the binding.",
                    );
                }
            }
        }
    }
    for fixture in &m.generated {
        let Some(bytes) = authorities.get(&fixture.authority) else {
            continue;
        };
        let expected = match fixture_bytes(bytes, &fixture.pointer) {
            Ok(bytes) => bytes,
            Err(e) => {
                finding(
                    report,
                    "fixture_source_invalid",
                    &fixture.path,
                    e,
                    "Repair the independent canonical JSON contract/pointer before generating anything.",
                );
                continue;
            }
        };
        match files::read(base, &fixture.path, &mut budget) {
            Ok(actual) => {
                report.observed.push(Observed {
                    path: fixture.path.clone(),
                    sha256: crate::contract::hash(&actual),
                });
                if actual != expected {
                    finding(
                        report,
                        "generated_fixture_drift",
                        &fixture.path,
                        "Fixture bytes differ from the declared canonical JSON projection",
                        "Run kritic staleness sync (dry run), inspect the source/projection, then --write; test/authority review pins are never auto-updated.",
                    );
                }
            }
            Err(e) => finding(
                report,
                "generated_fixture_unavailable",
                &fixture.path,
                e,
                "Review the declared source and use the explicit fixture sync; no arbitrary generator command is executed.",
            ),
        }
    }
}

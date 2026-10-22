//! Declared boundary coverage and guard/test bindings, not semantic invariant verification.
use std::collections::BTreeSet;
use std::path::Path;

use super::{FailurePolicy, GuardKind, InvariantEvidence, Manifest, Report, audit::finding, files};

pub(super) fn validate(m: &Manifest) -> Result<(), String> {
    if m.boundaries.len() > 100 || m.invariants.len() > 200 {
        return Err("At most 100 invariant boundaries and 200 invariants".into());
    }
    let mut ids = BTreeSet::new();
    for boundary in &m.boundaries {
        if boundary.id.trim().is_empty()
            || !ids.insert(&boundary.id)
            || boundary.risk.trim().is_empty()
            || boundary.risk.len() > 4096
            || boundary.required_invariants.len() > 100
            || !m.authorities.iter().any(|a| a.id == boundary.authority)
        {
            return Err("Invariant boundaries need unique IDs, known authority, bounded risk and required invariant IDs".into());
        }
    }
    ids.clear();
    for invariant in &m.invariants {
        if invariant.id.trim().is_empty()
            || !ids.insert(&invariant.id)
            || invariant.condition.trim().is_empty()
            || invariant.rationale.trim().is_empty()
            || invariant.condition.len() > 4096
            || invariant.rationale.len() > 4096
            || invariant.negative_tests.len() > 20
            || !m.boundaries.iter().any(|b| b.id == invariant.boundary)
        {
            return Err("Invariants need unique IDs, known boundary and bounded explicit condition/rationale/test bindings".into());
        }
        pattern(&invariant.enforcement_pattern)?;
        for test in &invariant.negative_tests {
            files::relative(&test.path)?;
            if test.purpose.trim().is_empty() || test.purpose.len() > 4096 {
                return Err("Negative test needs a bounded purpose".into());
            }
            pattern(&test.pattern)?;
        }
    }
    Ok(())
}

fn pattern(text: &str) -> Result<regex::Regex, String> {
    if text.trim().is_empty() || text.len() > 4096 {
        return Err("Invariant patterns must be nonempty and at most 4096 bytes".into());
    }
    let re = regex::Regex::new(text).map_err(|e| e.to_string())?;
    if re.is_match("")
        || ["unrelated", "123", "// placeholder"]
            .iter()
            .all(|sample| re.is_match(sample))
    {
        return Err("Vacuous invariant/test pattern cannot establish a binding".into());
    }
    Ok(re)
}

pub(super) fn check(base: &Path, m: &Manifest, report: &mut Report) {
    let mut budget = files::MAX_TOTAL;
    for boundary in &m.boundaries {
        if boundary.required_invariants.is_empty() {
            finding(
                report,
                "boundary_unprotected",
                &boundary.id,
                "Key boundary has no required invariants",
                "Name the state/input constraints and install appropriate release guards with negative tests.",
            );
        }
        for id in &boundary.required_invariants {
            if !m
                .invariants
                .iter()
                .any(|i| &i.id == id && i.boundary == boundary.id)
            {
                finding(
                    report,
                    "required_invariant_missing",
                    &boundary.id,
                    format!("Required invariant {id} has no definition for this boundary"),
                    "Define the condition, rationale, failure policy, actual guard and independent negative cases.",
                );
            }
        }
    }
    for invariant in &m.invariants {
        let Some(boundary) = m.boundaries.iter().find(|b| b.id == invariant.boundary) else {
            continue;
        };
        let Some(authority) = m.authorities.iter().find(|a| a.id == boundary.authority) else {
            continue;
        };
        let mut evidence = InvariantEvidence {
            id: invariant.id.clone(),
            boundary: boundary.id.clone(),
            condition: invariant.condition.clone(),
            rationale: invariant.rationale.clone(),
            boundary_risk: boundary.risk.clone(),
            external_input: boundary.external_input,
            required_in_release: boundary.required_in_release,
            failure_policy: format!("{:?}", invariant.failure_policy),
            enforcement_path: authority.path.clone(),
            enforcement_line: None,
            enforcement_excerpt: None,
            negative_test_paths: invariant
                .negative_tests
                .iter()
                .map(|t| t.path.clone())
                .collect(),
        };
        if boundary.required_in_release && invariant.guard_kind == GuardKind::DebugAssertion {
            finding(
                report,
                "debug_only_invariant",
                &invariant.id,
                "Required release invariant is declared as a debug-only assertion",
                "Install a release-path guard/type/schema constraint; keep developer assertions as supplementary checks.",
            );
        }
        if boundary.external_input && invariant.failure_policy == FailurePolicy::PanicInternal {
            finding(
                report,
                "panic_on_external_input",
                &invariant.id,
                "Externally controlled boundary declares an internal panic as failure handling",
                "Reject invalid external data explicitly; independently review any fatal-process behavior.",
            );
        }
        if matches!(
            invariant.condition.trim().to_ascii_lowercase().as_str(),
            "true" | "always" | "todo" | "tbd"
        ) {
            finding(
                report,
                "vacuous_invariant",
                &invariant.id,
                "Condition does not define a meaningful state/input restriction",
                "State a falsifiable predicate, units/range and why it must hold.",
            );
        }
        match files::read(base, &authority.path, &mut budget) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let matched = pattern(&invariant.enforcement_pattern)
                    .ok()
                    .and_then(|re| re.find(&text).map(|m| (m.start(), m.as_str().to_string())));
                if let Some((start, excerpt)) = matched {
                    if boundary.required_in_release
                        && excerpt.contains("debug_assert")
                        && invariant.guard_kind != GuardKind::DebugAssertion
                    {
                        finding(
                            report,
                            "debug_only_invariant",
                            &invariant.id,
                            "Matched guard uses debug_assert despite a release-guard declaration",
                            "Install an effective release guard; changing metadata does not change execution.",
                        );
                    }
                    evidence.enforcement_line =
                        Some(text[..start].bytes().filter(|c| *c == b'\n').count() + 1);
                    evidence.enforcement_excerpt = Some(excerpt.chars().take(512).collect());
                } else {
                    finding(
                        report,
                        "invariant_guard_missing",
                        &invariant.id,
                        format!("Declared guard not found in {}", authority.path),
                        "Trace the actual execution path and install/correct the guard binding; a comment or unreachable assertion is insufficient.",
                    );
                }
            }
            Err(e) => finding(
                report,
                "invariant_source_unavailable",
                &invariant.id,
                e,
                "Restore the actual canonical enforcement source.",
            ),
        }
        if invariant.negative_tests.is_empty() {
            finding(
                report,
                "invariant_negative_test_missing",
                &invariant.id,
                "No negative cases are bound to the invariant",
                "Add a test that violates the condition and asserts explicit rejection/rollback; test valid edge cases too.",
            );
        }
        for negative in &invariant.negative_tests {
            let mapped = m.tests.iter().any(|t| {
                t.path == negative.path && t.reviewed_inputs.contains_key(&boundary.authority)
            });
            if !mapped {
                finding(
                    report,
                    "invariant_test_unmapped",
                    &negative.path,
                    format!(
                        "Negative test does not bind enforcement authority {}",
                        boundary.authority
                    ),
                    "Map the actual negative test and its reviewed canonical inputs in this same manifest.",
                );
            }
            match files::read(base, &negative.path, &mut budget) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    if !pattern(&negative.pattern).is_ok_and(|re| re.is_match(&text)) {
                        finding(
                            report,
                            "invariant_negative_case_missing",
                            &negative.path,
                            format!("Negative-case binding missing for {}", invariant.id),
                            "Restore the meaningful negative case and run it; do not weaken the pattern to match unrelated text.",
                        );
                    }
                }
                Err(e) => finding(
                    report,
                    "invariant_negative_case_unavailable",
                    &negative.path,
                    e,
                    "Restore and execute the actual negative test.",
                ),
            }
        }
        report.invariant_evidence.push(evidence);
    }
    if !m.invariants.is_empty() {
        report.review_notes.push("Invariant findings/line bindings are structural leads only: verify predicates/rationale, units, state transitions, actual release-path reachability and failing/passing boundary cases independently. A regex can match comments or dead code.".into());
    }
}

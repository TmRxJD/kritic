use super::evidence::Checked;
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn add(findings: &mut Vec<Finding>, code: &str, location: &str, detail: impl Into<String>) {
    findings.push(Finding {
        code: code.into(),
        location: location.into(),
        detail: detail.into(),
    });
}
fn observed<'a>(id: &str, checked: &'a BTreeMap<String, Checked>) -> Option<&'a serde_json::Value> {
    checked
        .get(id)
        .filter(|e| e.result.verified && e.result.kind == EvidenceKind::Observation)?
        .document
        .as_ref()
}
fn has_observations(ids: &[String], checked: &BTreeMap<String, Checked>) -> bool {
    !ids.is_empty() && ids.iter().all(|id| observed(id, checked).is_some())
}
fn signatures(case: &Case, ids: &[String]) -> Vec<String> {
    let mut values: Vec<_> = ids
        .iter()
        .filter_map(|id| case.evidence.iter().find(|item| &item.id == id))
        .map(|item| format!("{}:{}", item.path, item.sha256))
        .collect();
    values.sort();
    values.dedup();
    values
}
fn asserted(case: &Case, id: &str, pointer: &str, expected: &serde_json::Value) -> bool {
    case.evidence
        .iter()
        .find(|item| item.id == id)
        .is_some_and(|item| {
            item.assertions
                .iter()
                .any(|assertion| assertion.pointer == pointer && &assertion.expected == expected)
        })
}

pub(super) fn assess(case: &Case, checked: &BTreeMap<String, Checked>) -> Vec<Finding> {
    let mut findings = vec![];
    if case.hypotheses.is_empty() {
        add(
            &mut findings,
            "missing_hypothesis",
            "hypotheses",
            "State a falsifiable working hypothesis; an empty list is not a completed diagnosis record",
        );
    }
    if case.problem.expected.trim() == case.problem.observed.trim() {
        add(
            &mut findings,
            "problem_not_distinguished",
            "problem",
            "Expected and observed descriptions are identical; name a falsifiable discrepancy",
        );
    }
    if !has_observations(&case.problem.failure_observations, checked)
        || !case.problem.failure_observations.iter().any(|id| {
            observed(id, checked).is_some_and(|document| {
                document.pointer("/data/symptom_persists") == Some(&serde_json::Value::Bool(true))
            }) && asserted(
                case,
                id,
                "/data/symptom_persists",
                &serde_json::Value::Bool(true),
            )
        })
    {
        add(
            &mut findings,
            "missing_reproduction_evidence",
            "problem",
            "Original failure needs current pinned observation content, not a hypothesis or a source-file hash",
        );
    }
    for stage in &case.stages {
        if stage.owner.is_none() {
            add(
                &mut findings,
                "missing_owner",
                &stage.id,
                "Declare the stage owner; responsibility is otherwise unknown",
            );
        }
        if stage.sources.is_empty()
            || !stage.sources.iter().all(|id| {
                checked
                    .get(id)
                    .is_some_and(|e| e.result.verified && e.result.kind == EvidenceKind::Source)
            })
        {
            add(
                &mut findings,
                "missing_stage_source",
                &stage.id,
                "Bind the stage to current pinned source evidence",
            );
        }
        if stage.schemas.is_empty() {
            if stage.schema_not_applicable.is_none() {
                add(
                    &mut findings,
                    "schema_boundary_unknown",
                    &stage.id,
                    "Declare applicable schemas or explain why no schema boundary applies",
                );
            }
        } else {
            if stage.schema_not_applicable.is_some() {
                add(
                    &mut findings,
                    "schema_applicability_conflict",
                    &stage.id,
                    "Schema bindings and a not-applicable declaration conflict",
                );
            }
            for schema in &stage.schemas {
                let pinned = checked
                    .get(schema)
                    .is_some_and(|e| e.result.verified && e.result.kind == EvidenceKind::Schema);
                let enforced = stage.enforcement_observations.iter().any(|id| {
                    observed(id, checked).is_some_and(|doc| {
                        doc.pointer("/data/stage")
                            .and_then(serde_json::Value::as_str)
                            == Some(stage.id.as_str())
                            && doc
                                .pointer("/data/schema")
                                .and_then(serde_json::Value::as_str)
                                == Some(schema.as_str())
                            && doc.pointer("/data/enforced") == Some(&serde_json::Value::Bool(true))
                    }) && asserted(case, id, "/data/enforced", &serde_json::Value::Bool(true))
                });
                if !pinned || !enforced {
                    add(
                        &mut findings,
                        "schema_enforcement_unverified",
                        &stage.id,
                        format!(
                            "{schema}: file presence is not runtime enforcement; supply an observation bound to /data/stage and /data/schema"
                        ),
                    );
                }
            }
        }
    }
    let mut reached = BTreeSet::from([case.entry_stage.clone()]);
    let mut valid_links = vec![];
    for link in &case.links {
        let witnessed = link.observations.iter().any(|id| {
            observed(id, checked).is_some_and(|doc| {
                doc.pointer("/data/from")
                    .and_then(serde_json::Value::as_str)
                    == Some(link.from.as_str())
                    && doc.pointer("/data/to").and_then(serde_json::Value::as_str)
                        == Some(link.to.as_str())
            }) && asserted(case, id, "/data/from", &serde_json::json!(link.from))
                && asserted(case, id, "/data/to", &serde_json::json!(link.to))
        });
        if witnessed {
            valid_links.push(link);
        } else {
            add(
                &mut findings,
                "unobserved_link",
                &format!("{}->{}", link.from, link.to),
                "Need a verified observation bound to this edge's /data/from and /data/to; a generic reused observation is not a trace",
            );
        }
    }
    loop {
        let old = reached.len();
        for link in &valid_links {
            if reached.contains(&link.from) {
                reached.insert(link.to.clone());
            }
        }
        if reached.len() == old {
            break;
        }
    }
    let mut reaches_symptom = BTreeSet::from([case.symptom_stage.clone()]);
    loop {
        let old = reaches_symptom.len();
        for link in &valid_links {
            if reaches_symptom.contains(&link.to) {
                reaches_symptom.insert(link.from.clone());
            }
        }
        if reaches_symptom.len() == old {
            break;
        }
    }
    if !reached.contains(&case.symptom_stage) {
        add(
            &mut findings,
            "incomplete_trace",
            "links",
            "No observed entry-to-symptom path exists in the supplied record",
        );
    }
    for configuration in &case.configuration {
        if configuration.consumers.is_empty()
            || case
                .stages
                .iter()
                .find(|stage| stage.id == configuration.owner_stage)
                .is_none_or(|stage| stage.owner.is_none())
        {
            add(
                &mut findings,
                "configuration_ownership_unknown",
                &configuration.key,
                "Declare the canonical resolver owner_stage with an owner and explicit consumer stages",
            );
        }
        if !reached.contains(&configuration.owner_stage)
            || configuration
                .consumers
                .iter()
                .any(|consumer| !reached.contains(consumer))
        {
            add(
                &mut findings,
                "configuration_handoff_unobserved",
                &configuration.key,
                "Resolver/consumers are outside the observed entry-to-symptom trace",
            );
        }
        let mut effective = None;
        let mut complete = !configuration.sources.is_empty();
        for source in &configuration.sources {
            let value =
                observed(&source.evidence, checked).and_then(|doc| doc.pointer(&source.pointer));
            if let Some(value) = value {
                effective = Some(value);
            } else {
                complete = false;
            }
        }
        if !complete {
            add(
                &mut findings,
                "configuration_source_missing",
                &configuration.key,
                "Resolve every declared source with current observation content; order is caller-declared low-to-high precedence",
            );
        } else if effective != Some(&configuration.effective) {
            add(
                &mut findings,
                "configuration_precedence_conflict",
                &configuration.key,
                "Declared effective value disagrees with the last ordered source's actual observed value",
            );
        }
    }
    for hypothesis in &case.hypotheses {
        if hypothesis
            .stages
            .iter()
            .any(|stage| !reached.contains(stage) || !reaches_symptom.contains(stage))
        {
            add(
                &mut findings,
                "hypothesis_outside_trace",
                &hypothesis.id,
                "An implicated stage is outside the observed entry-to-symptom corridor; unrelated/disconnected or dead-branch sources cannot justify the patch",
            );
        }
        if hypothesis.stages.is_empty() || !has_observations(&hypothesis.support, checked) {
            add(
                &mut findings,
                "unsupported_hypothesis",
                &hypothesis.id,
                "Name the implicated stage and checked observation support; prose/source pins alone are not diagnosis evidence",
            );
        }
        if has_observations(&hypothesis.counterevidence, checked) {
            add(
                &mut findings,
                "counterevidence_requires_resolution",
                &hypothesis.id,
                "Verified counterevidence is declared; assess whether it refutes or limits this hypothesis before patching",
            );
        }
        if hypothesis.experiments.is_empty() {
            add(
                &mut findings,
                "missing_discriminating_experiment",
                &hypothesis.id,
                "Declare a prediction and a different alternative prediction, with a bounded experiment",
            );
        }
        for experiment in &hypothesis.experiments {
            if experiment.prediction.trim() == experiment.alternative_prediction.trim() {
                add(
                    &mut findings,
                    "nondiscriminating_experiment",
                    &experiment.id,
                    "Predictions are identical; this experiment cannot distinguish the declared alternatives",
                );
            }
            if experiment.outcome == ExperimentOutcome::NotRun
                || !has_observations(&experiment.observations, checked)
            {
                add(
                    &mut findings,
                    "experiment_unverified",
                    &experiment.id,
                    "Experiment not run or lacks current checked observations; outcome remains caller-declared",
                );
            }
            if experiment.outcome == ExperimentOutcome::Refutes
                && has_observations(&experiment.observations, checked)
            {
                add(
                    &mut findings,
                    "hypothesis_refuted",
                    &hypothesis.id,
                    "Declared experiment outcome refutes this hypothesis; reframe rather than patching under that claim",
                );
            }
        }
    }
    let mut failed = BTreeMap::<(&str, Vec<String>), usize>::new();
    for attempt in &case.attempts {
        if !has_observations(&attempt.observations, checked) {
            add(
                &mut findings,
                "attempt_unverified",
                &attempt.id,
                "Attempt outcome lacks current content-checked observations",
            );
        }
        if attempt.outcome == AttemptOutcome::Failed {
            let observations = signatures(case, &attempt.observations);
            *failed
                .entry((&attempt.hypothesis, observations))
                .or_default() += 1;
        }
        if attempt.outcome == AttemptOutcome::Succeeded {
            let reproduced = attempt
                .observations
                .iter()
                .filter_map(|id| observed(id, checked))
                .any(|doc| {
                    doc.pointer("/data/symptom_persists") == Some(&serde_json::Value::Bool(false))
                });
            if !reproduced {
                add(
                    &mut findings,
                    "success_not_revalidated",
                    &attempt.id,
                    "A success label needs original reproduction evidence with /data/symptom_persists=false; it is not established by a passing unrelated assertion",
                );
            }
        }
    }
    for ((hypothesis, observations), count) in failed {
        let informative = case
            .hypotheses
            .iter()
            .find(|item| item.id == hypothesis)
            .is_some_and(|item| {
                item.experiments.iter().any(|experiment| {
                    matches!(
                        experiment.outcome,
                        ExperimentOutcome::Supports | ExperimentOutcome::Refutes
                    ) && experiment.prediction.trim() != experiment.alternative_prediction.trim()
                        && has_observations(&experiment.observations, checked)
                        && signatures(case, &experiment.observations)
                            .iter()
                            .any(|signature| !observations.contains(signature))
                })
            });
        if count >= 2 && !informative {
            add(
                &mut findings,
                "repeated_failed_attempts",
                hypothesis,
                format!(
                    "{count} failed attempts reuse the same hypothesis/evidence without a new informative experiment; reframe before retrying"
                ),
            );
        }
    }
    if let Some(patch) = &case.proposed_patch {
        if patch.paths.is_empty() || patch.max_changed_lines == 0 {
            add(
                &mut findings,
                "patch_unbounded",
                "proposed_patch",
                "Declare concrete paths and a nonzero maximum changed-line budget",
            );
        }
        if patch.paths.len() > 4 || patch.max_changed_lines > 200 {
            add(
                &mut findings,
                "patch_scope_review",
                "proposed_patch",
                "Patch spans a broad review unit; justify the minimum causal boundary rather than scattering speculative changes",
            );
        }
        let hypothesis = case
            .hypotheses
            .iter()
            .find(|h| h.id == patch.hypothesis)
            .expect("validated reference");
        let bound_paths: BTreeSet<&str> = case
            .stages
            .iter()
            .filter(|s| hypothesis.stages.contains(&s.id))
            .flat_map(|s| &s.sources)
            .filter_map(|id| {
                case.evidence
                    .iter()
                    .find(|e| &e.id == id)
                    .map(|e| e.path.as_str())
            })
            .collect();
        if patch
            .paths
            .iter()
            .any(|path| !bound_paths.contains(path.as_str()))
        {
            add(
                &mut findings,
                "patch_boundary_unverified",
                "proposed_patch",
                "Some proposed paths are outside the hypothesis stage's pinned source boundary; explain necessary new files/cross-stage changes before editing",
            );
        }
    }
    for unknown in &case.unknowns {
        add(&mut findings, "declared_unknown", "unknowns", unknown);
    }
    findings
}

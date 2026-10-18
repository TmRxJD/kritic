//! Deterministic debugging-case consistency checks, never proof of a root cause.
mod assess;
mod evidence;
mod types;
mod validate;
use std::path::Path;
pub use types::*;

pub fn schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Case)).expect("schema serialization")
}

pub fn execute(request: &Request) -> Result<Report, String> {
    run(Path::new(&request.base_dir), request.case_path.as_deref())
}

fn run(base: &Path, case_path: Option<&str>) -> Result<Report, String> {
    let base = base
        .canonicalize()
        .map_err(|error| format!("Framing base_dir: {error}"))?;
    if !base.is_dir() {
        return Err("Framing base_dir must be a directory".into());
    }
    let path = case_path.unwrap_or(".kritic/debug-case.json");
    let bytes = evidence::read(&base, path, 1024 * 1024)?;
    let case: Case = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Malformed framing case: {error}"))?;
    validate::validate(&case)?;
    let mut budget = 8 * 1024 * 1024;
    let checked: std::collections::BTreeMap<_, _> = case
        .evidence
        .iter()
        .map(|item| {
            (
                item.id.clone(),
                evidence::check(&base, item, &case, &mut budget),
            )
        })
        .collect();
    let mut findings = assess::assess(&case, &checked);
    for item in checked.values().filter(|item| !item.result.verified) {
        findings.push(Finding {
            code: format!("evidence_{}", item.result.state),
            location: item.result.id.clone(),
            detail: item.result.detail.clone(),
        });
    }
    let contradicted = findings.iter().any(|finding| {
        matches!(
            finding.code.as_str(),
            "configuration_precedence_conflict"
                | "hypothesis_refuted"
                | "problem_not_distinguished"
                | "schema_applicability_conflict"
                | "evidence_contradicted"
        )
    });
    let stale = checked.values().any(|item| item.result.state == "stale");
    let status = if contradicted {
        "contradicted"
    } else if stale {
        "stale"
    } else if findings.is_empty() {
        "consistent"
    } else {
        "incomplete"
    };
    let next_step = if stale {
        "Refresh pinned observations and reproduce the original failure against the intended revision/run/input corpus."
    } else if findings.iter().any(|f| {
        matches!(
            f.code.as_str(),
            "hypothesis_refuted"
                | "repeated_failed_attempts"
                | "counterevidence_requires_resolution"
        )
    }) {
        "Reframe the retained hypothesis and run a discriminating alternative before another patch."
    } else if findings
        .iter()
        .any(|f| f.code == "configuration_precedence_conflict")
    {
        "Resolve observed configuration precedence and reproduce the failure with the actual effective value."
    } else if findings
        .iter()
        .any(|f| matches!(f.code.as_str(), "incomplete_trace" | "unobserved_link"))
    {
        "Capture the missing observed handoff from entry to symptom before choosing a patch boundary."
    } else if !findings.is_empty() {
        "Resolve the listed evidence/ownership/experiment gaps and rerun framing before a bounded patch review."
    } else {
        "Independently review the supplied observations and alternative predictions, then review only the bounded proposed patch; consistency is not root-cause proof."
    };
    let observations: Vec<_> = checked
        .iter()
        .filter_map(|(id, item)| {
            item.document
                .as_ref()
                .filter(|_| item.result.kind == EvidenceKind::Observation)
                .map(|document| serde_json::json!({"evidence_id":id,"data":document["data"]}))
        })
        .collect();
    let review_packet = serde_json::json!({
        "checked_observation_contents":observations,
        "limits":["Only supplied file hashes/data assertions/context agreement checked; no actual HEAD comparison or execution authenticity attestation", "Observed handoff records do not prove causality; schema bindings do not attest enforcement execution", "Configuration precedence, ownership, experiment outcomes and diagnosis remain caller declarations requiring independent review"],
        "declared_case":case,
    });
    let evidence: Vec<_> = checked.into_values().map(|item| item.result).collect();
    Ok(Report {tool:"framing_critic".into(),status:status.into(),deterministic_scope:"Internal consistency of the supplied bounded record only; no approval, execution attestation, current-HEAD verification or root-cause proof".into(),next_step:next_step.into(),finding_count:findings.len(),evidence_count:evidence.len(),findings,evidence,review_packet,full_result:None})
}

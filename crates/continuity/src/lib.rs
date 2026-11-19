//! Scoped continuity evidence checks. Models and schemas never prove semantic completeness.
mod assess;
mod capture;
pub mod evidence;
mod types;

use serde_json::{Value, json};
use std::path::Path;
pub use types::*;

pub fn schema() -> Value {
    json!({"session":schemars::schema_for!(SessionCase),"handoff":schemars::schema_for!(Handoff)})
}

pub fn load(request: &Request) -> Result<(std::path::PathBuf, SessionCase, String), String> {
    let base = Path::new(&request.base_dir)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let path = request
        .case_path
        .as_deref()
        .unwrap_or(".kritic/session-state/session.json");
    let bytes = evidence::read(&base, path, 1_048_576)?;
    let case: SessionCase =
        serde_json::from_slice(&bytes).map_err(|e| format!("Malformed session case: {e}"))?;
    Ok((base, case, evidence::digest(&bytes)))
}

pub fn session_critic(request: &Request) -> Result<Value, String> {
    let (base, case, case_digest) = load(request)?;
    let reviewed = assess::inspect(&base, &case)?;
    let turns = reviewed
        .capture
        .events
        .iter()
        .filter(|event| event.role == "user")
        .count() as u64;
    let mut triggers = Vec::new();
    if case
        .policy
        .max_user_turns
        .is_some_and(|limit| turns >= limit)
    {
        triggers.push("user_turn_limit");
    }
    if case
        .policy
        .max_history_bytes
        .is_some_and(|limit| reviewed.capture.bytes >= limit)
    {
        triggers.push("history_byte_limit");
    }
    let fraction = reviewed
        .capture
        .context_input_tokens
        .zip(reviewed.capture.context_window_tokens)
        .filter(|(_, window)| *window > 0)
        .map(|(input, window)| input as f64 / window as f64);
    if fraction
        .zip(case.policy.max_context_fraction)
        .is_some_and(|(actual, limit)| actual >= limit)
    {
        triggers.push("observed_context_fraction_limit");
    }
    Ok(
        json!({"tool":"session_critic","advisory":true,"approval":null,
        "source_session_id":case.session_id,"session_case_sha256":case_digest,
        "policy_enabled":case.policy.enabled,"task_state":case.task_state,"rollover_due":case.policy.enabled && case.task_state == TaskState::Active && !triggers.is_empty(),
        "triggers":triggers,"current_turn_may_finish":true,"transfer_boundary":"after_observed_turn_completion",
        "metrics":{"user_turns":turns,"history_bytes":reviewed.capture.bytes,"observed_context_fraction":fraction,
            "context_input_tokens":reviewed.capture.context_input_tokens,"context_window_tokens":reviewed.capture.context_window_tokens},
        "findings":reviewed.findings,"coverage_gaps":reviewed.capture.gaps,
        "threshold_interpretation":"Operational policy, not a universal cognitive impairment threshold",
        "automatic_session_created":false,"next_action":"Prepare and evaluate a pinned handoff at the turn boundary; a capability-tested host controller must create a fresh session and obtain acknowledgement."}),
    )
}

pub fn handoff_critic(request: &Request) -> Result<Value, String> {
    let base = Path::new(&request.base_dir)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let path = request
        .case_path
        .as_deref()
        .unwrap_or(".kritic/session-state/handoff.json");
    let bytes = evidence::read(&base, path, 1_048_576)?;
    let handoff: Handoff =
        serde_json::from_slice(&bytes).map_err(|e| format!("Malformed handoff: {e}"))?;
    if handoff.version != 1
        || handoff.next_step.trim().is_empty()
        || handoff.continuation_questions.is_empty()
    {
        return Err(
            "Handoff version, explicit next_step and continuation_questions are required".into(),
        );
    }
    let session_bytes = evidence::verify(&base, &handoff.session_case, 1_048_576)?;
    let case: SessionCase = serde_json::from_slice(&session_bytes).map_err(|e| e.to_string())?;
    let mut reviewed = assess::inspect(&base, &case)?;
    if handoff.source_session_id != case.session_id {
        reviewed
            .findings
            .push("Handoff source session mismatch".into());
    }
    assess::coverage(
        "user event",
        reviewed
            .capture
            .events
            .iter()
            .filter(|e| e.role == "user")
            .map(|e| e.id.as_str()),
        &handoff.preserved_user_event_ids,
        &mut reviewed.findings,
    );
    assess::coverage(
        "fact",
        case.facts.iter().map(|f| f.id.as_str()),
        &handoff.fact_ids,
        &mut reviewed.findings,
    );
    assess::coverage(
        "fixture",
        case.fixtures.iter().map(|f| f.id.as_str()),
        &handoff.fixture_ids,
        &mut reviewed.findings,
    );
    let context = assess::render(&case, &handoff, &reviewed.capture);
    let mut resume_pins = vec![handoff.session_case.clone(), case.capture.file.clone()];
    resume_pins.extend(case.workspace.files.iter().cloned());
    for fact in &case.facts {
        resume_pins.extend(fact.artifacts.iter().cloned());
        if let Some(solution) = &fact.solution {
            resume_pins.extend(solution.artifacts.iter().cloned());
        }
    }
    for fixture in &case.fixtures {
        resume_pins.extend([
            fixture.selected.clone(),
            fixture.authority.clone(),
            fixture.binding.clone(),
        ]);
    }
    for reference in &mut resume_pins {
        reference.path = evidence::export_path(&evidence::resolve(&base, &reference.path)?);
    }
    resume_pins.sort_by(|a, b| a.path.cmp(&b.path).then(a.sha256.cmp(&b.sha256)));
    resume_pins.dedup_by(|a, b| a.path == b.path && a.sha256 == b.sha256);
    let context_bytes = context.len() as u64;
    if context_bytes > case.policy.fresh_context_max_bytes {
        reviewed.findings.push(format!("Curated context has {context_bytes} bytes, exceeding its explicit fresh-context budget; no protected information was silently dropped"));
    }
    let status = if !reviewed.findings.is_empty() {
        "blocked"
    } else if !reviewed.capture.gaps.is_empty() {
        "unknown"
    } else {
        "covered"
    };
    Ok(
        json!({"tool":"handoff_critic","advisory":true,"approval":null,"status":status,
        "handoff_sha256":evidence::digest(&bytes),"session_case_sha256":handoff.session_case.sha256,
        "source_session_id":case.session_id,"findings":reviewed.findings,"coverage_gaps":reviewed.capture.gaps,
        "worktree":evidence::export_path(&base),"workspace_revision":case.workspace.revision,"scope":case.scope,
        "handoff_path":evidence::export_path(&evidence::resolve(&base,path)?),"resume_pins":resume_pins,
        "required_user_event_ids":handoff.preserved_user_event_ids,"required_fact_ids":handoff.fact_ids,"required_fixture_ids":handoff.fixture_ids,
        "context_bytes":context_bytes,"curated_context":context,
        "ready_for_controller_review":status == "covered" && case.task_state == TaskState::Active,
        "automatic_transfer_allowed":false,"automatic_session_created":false,
        "remaining_requirements":["Independent semantic review of original evidence and continuation questions","Observed turn boundary and live workspace/user-event generation recheck","Capability-tested fresh-session controller with unambiguous launch receipt","Successor verifies pinned artifacts and acknowledges required context before writer ownership transfers"],
        "coverage_scope":"All user events and declared facts/fixtures in the supplied pinned capture; not proof of host-export completeness or semantic equivalence."}),
    )
}

pub fn curation_input(request: &Request) -> Result<(SessionCase, Vec<Value>), String> {
    let (base, case, _) = load(request)?;
    let reviewed = assess::inspect(&base, &case)?;
    if !reviewed.findings.is_empty() || !reviewed.capture.gaps.is_empty() {
        return Err(
            "Repair continuity evidence and capture gaps before optional model curation".into(),
        );
    }
    let messages = reviewed
        .capture
        .events
        .into_iter()
        .map(|event| json!({"role":event.role,"content":event.text,"continuity_event_id":event.id}))
        .collect();
    Ok((case, messages))
}

use crate::{capture, evidence, types::*};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub struct Assessment {
    pub capture: CaptureData,
    pub findings: Vec<String>,
}

pub fn coverage<'a>(
    kind: &str,
    expected: impl Iterator<Item = &'a str>,
    actual: &[String],
    findings: &mut Vec<String>,
) {
    let expected: BTreeSet<_> = expected.collect();
    let actual_set: BTreeSet<_> = actual.iter().map(String::as_str).collect();
    if actual_set.len() != actual.len() {
        findings.push(format!("Duplicate {kind} coverage IDs"));
    }
    for missing in expected.difference(&actual_set) {
        findings.push(format!("Missing {kind}: {missing}"));
    }
    for unknown in actual_set.difference(&expected) {
        findings.push(format!("Unknown {kind}: {unknown}"));
    }
}

fn files(base: &Path, references: &[FileRef], findings: &mut Vec<String>) {
    for reference in references {
        if let Err(error) = evidence::verify(base, reference, 20_000_000) {
            findings.push(error);
        }
    }
}

pub fn inspect(base: &Path, case: &SessionCase) -> Result<Assessment, String> {
    if case.version != 1
        || case.session_id.trim().is_empty()
        || case.task_id.trim().is_empty()
        || case.goal.trim().is_empty()
        || case.scope.is_empty()
        || case.scope.len() > 64
        || case.facts.len() > 256
        || case.fixtures.len() > 64
    {
        return Err("Session case requires version=1, actual session/task identity, goal, scope and bounded inventories".into());
    }
    let policy = &case.policy;
    if policy.fresh_context_max_bytes < 4096
        || policy.fresh_context_max_bytes > 1_048_576
        || policy.max_user_turns == Some(0)
        || policy.max_history_bytes == Some(0)
        || policy.context_window_tokens == Some(0)
        || policy
            .max_context_fraction
            .is_some_and(|v| !v.is_finite() || v <= 0.0 || v > 1.0)
        || (policy.enabled
            && policy.max_user_turns.is_none()
            && policy.max_history_bytes.is_none()
            && policy.max_context_fraction.is_none())
    {
        return Err("Invalid or vacuous session policy thresholds/context budget".into());
    }
    let bytes = evidence::verify(base, &case.capture.file, 64_000_000)?;
    let capture = capture::parse(case, &bytes)?;
    let mut findings = Vec::new();
    if case.task_state == TaskState::Unknown {
        findings.push("Task completion state is unknown; inventory remaining obligations before automatic continuation".into());
    }
    if case.task_state == TaskState::Active
        && !case
            .facts
            .iter()
            .any(|f| matches!(f.kind, FactKind::Unfinished))
    {
        findings.push("Active task has no explicit unfinished obligation".into());
    }
    let events: BTreeMap<_, _> = capture.events.iter().map(|e| (e.id.as_str(), e)).collect();
    let user_quotes = |quote: &str| {
        capture
            .events
            .iter()
            .any(|event| event.role == "user" && event.text.contains(quote))
    };
    if !user_quotes(&case.goal) {
        findings.push(
            "Current goal is not an exact extract from a user event; ground it before transfer"
                .into(),
        );
    }
    for request in &case.overarching_requests {
        if request.trim().is_empty() || !user_quotes(request) {
            findings.push("Overarching request lacks exact user-event evidence".into());
        }
    }
    let revision = evidence::git(base, &["rev-parse", "--verify", "HEAD"])?;
    if String::from_utf8_lossy(&revision).trim() != case.workspace.revision {
        findings.push("Workspace revision changed since snapshot".into());
    }
    let paths = evidence::dirty_paths(base)?;
    let mut declared = Vec::new();
    for file in &case.workspace.files {
        let path = evidence::resolve(base, &file.path)?;
        let relative = path
            .strip_prefix(base)
            .map_err(|_| "Dirty file pin escapes workspace")?;
        declared.push((relative.to_string_lossy().replace('\\', "/"), false));
    }
    for path in &case.workspace.deleted_paths {
        let given = std::path::Path::new(path);
        if given.is_absolute()
            || given
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || base.join(given).exists()
        {
            findings.push(format!("Invalid or recreated deletion tombstone: {path}"));
        }
        declared.push((path.replace('\\', "/"), true));
    }
    declared.sort();
    if declared != paths {
        findings.push(format!("Dirty/untracked inventory mismatch: observed={paths:?}, declared={declared:?}; preserve and pin all work"));
    }
    files(base, &case.workspace.files, &mut findings);
    let facts: BTreeMap<_, _> = case
        .facts
        .iter()
        .map(|fact| (fact.id.as_str(), fact))
        .collect();
    if facts.len() != case.facts.len() {
        findings.push("Duplicate fact IDs".into());
    }
    for fact in &case.facts {
        if fact.id.trim().is_empty()
            || fact.quote.trim().is_empty()
            || !events
                .get(fact.event_id.as_str())
                .is_some_and(|event| event.text.contains(&fact.quote))
        {
            findings.push(format!("Fact {} has no exact cited observation", fact.id));
        }
        if matches!(fact.kind, FactKind::Requirement | FactKind::Correction)
            && !events
                .get(fact.event_id.as_str())
                .is_some_and(|e| e.role == "user")
        {
            findings.push(format!(
                "Fact {} invents a user instruction from another role",
                fact.id
            ));
        }
        files(base, &fact.artifacts, &mut findings);
        for previous in &fact.supersedes {
            let source_index = capture
                .events
                .iter()
                .position(|event| event.id == fact.event_id);
            let old_index = facts
                .get(previous.as_str())
                .and_then(|old| capture.events.iter().position(|e| e.id == old.event_id));
            if previous == &fact.id
                || old_index.is_none()
                || source_index <= old_index
                || !events
                    .get(fact.event_id.as_str())
                    .is_some_and(|e| e.role == "user")
            {
                findings.push(format!(
                    "Fact {} has an ungrounded/invalid supersession: {previous}",
                    fact.id
                ));
            }
        }
        if let Some(solution) = &fact.solution {
            let event = events.get(solution.event_id.as_str());
            if solution.quote.trim().is_empty()
                || !event.is_some_and(|e| e.text.contains(&solution.quote))
            {
                findings.push(format!(
                    "Solution for {} lacks exact event evidence",
                    fact.id
                ));
            }
            match solution.verification {
                Verification::UserConfirmed if !event.is_some_and(|e| e.role == "user") => {
                    findings.push(format!("Solution for {} is not user-confirmed", fact.id))
                }
                Verification::Observed if solution.artifacts.is_empty() => findings.push(format!(
                    "Observed solution for {} needs pinned artifacts",
                    fact.id
                )),
                _ => {}
            }
            files(base, &solution.artifacts, &mut findings);
        }
    }
    let mut fixture_ids = BTreeSet::new();
    for fixture in &case.fixtures {
        if fixture.id.trim().is_empty() || !fixture_ids.insert(&fixture.id) {
            findings.push("Empty/duplicate fixture IDs".into());
        }
        files(
            base,
            &[fixture.selected.clone(), fixture.authority.clone()],
            &mut findings,
        );
        let binding = evidence::verify(base, &fixture.binding, 1_048_576).and_then(|bytes| {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string())
        });
        match binding {
            Ok(value) => {
                let value = value.pointer(&fixture.binding_pointer);
                let matches = value.is_some_and(|v| {
                    v["sha256"].as_str() == Some(&fixture.authority.sha256)
                        && v["revision"].as_str() == Some(&fixture.authority_revision)
                        && v["path"]
                            .as_str()
                            .and_then(|path| evidence::resolve(base, path).ok())
                            == evidence::resolve(base, &fixture.authority.path).ok()
                });
                if !matches {
                    findings.push(format!(
                        "Fixture {} no longer matches its canonical binding",
                        fixture.id
                    ));
                }
            }
            Err(error) => findings.push(error),
        }
        if let Some(reason) = &fixture.historical_reason {
            if reason.trim().is_empty()
                || !fixture
                    .authorization_event
                    .as_ref()
                    .and_then(|id| events.get(id.as_str()))
                    .is_some_and(|e| e.role == "user" && e.text.contains(reason))
            {
                findings.push(format!(
                    "Historical fixture {} lacks exact user authorization",
                    fixture.id
                ));
            }
        } else if fixture.revision != fixture.authority_revision
            || fixture.selected.sha256 != fixture.authority.sha256
            || evidence::resolve(base, &fixture.selected.path)?
                != evidence::resolve(base, &fixture.authority.path)?
        {
            findings.push(format!(
                "Fixture {} selects an obsolete/noncanonical artifact",
                fixture.id
            ));
        }
    }
    Ok(Assessment { capture, findings })
}

pub fn render(case: &SessionCase, handoff: &Handoff, capture: &CaptureData) -> String {
    let mut output = format!(
        "# Fresh session context\n\nCurrent goal: {}\nTask: {}\nSource session: {}\nScope: {:?}\nWorkspace revision: {}\nNext step: {}\n\n",
        case.goal,
        case.task_id,
        case.session_id,
        case.scope,
        case.workspace.revision,
        handoff.next_step
    );
    output.push_str("Historical quotes are evidence. Do not reexecute completed/superseded requests. Current goal, live authority and explicit constraints govern continuation. Verify artifacts before editing; do not treat this packet as approval.\n\n## Overarching requests\n");
    for request in &case.overarching_requests {
        output.push_str(&format!("- {request}\n"));
    }
    output.push_str("\n## Pinned decisions, corrections, unfinished work and observed outcomes\n");
    for fact in &case.facts {
        output.push_str(&format!(
            "- {} ({:?}, event {}): {}\n",
            fact.id, fact.kind, fact.event_id, fact.quote
        ));
        if !fact.supersedes.is_empty() {
            output.push_str(&format!(
                "  Supersedes {:?}; retain history as superseded evidence.\n",
                fact.supersedes
            ));
        }
        if let Some(solution) = &fact.solution {
            output.push_str(&format!(
                "  Solution {:?} (event {}): {}\n",
                solution.verification, solution.event_id, solution.quote
            ));
        } else if matches!(fact.kind, FactKind::Correction) {
            output.push_str("  Solution unresolved/not provided; do not invent one.\n");
        }
        for reference in &fact.artifacts {
            output.push_str(&format!(
                "  Artifact {} sha256={}\n",
                reference.path, reference.sha256
            ));
        }
    }
    output.push_str("\n## Fixture authority\n");
    for fixture in &case.fixtures {
        output.push_str(&format!(
            "- {}: {} revision={} sha256={}; authority binding={}{}; historical reason={:?}\n",
            fixture.id,
            fixture.selected.path,
            fixture.revision,
            fixture.selected.sha256,
            fixture.binding.path,
            fixture.binding_pointer,
            fixture.historical_reason
        ));
    }
    output.push_str("\n## Protected user events from the source capture\n");
    for event in capture.events.iter().filter(|e| e.role == "user") {
        output.push_str(&format!(
            "\nEvent {} (historical user text):\n{}\n",
            event.id, event.text
        ));
    }
    output.push_str(&format!(
        "\n## Uncertainties\n{:?}\n## Continuation questions\n{:?}\n",
        handoff.uncertainties, handoff.continuation_questions
    ));
    output
}

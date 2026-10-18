use super::*;
use std::collections::BTreeSet;

fn nonempty(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 8192 && !value.contains('\0')
}
fn ids<'a>(values: impl Iterator<Item = &'a str>) -> Result<BTreeSet<String>, String> {
    let mut seen = BTreeSet::new();
    for id in values {
        if id.is_empty()
            || id.len() > 128
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
            || !seen.insert(id.to_string())
        {
            return Err(format!(
                "IDs must be unique nonempty ASCII identifiers: {id:?}"
            ));
        }
    }
    Ok(seen)
}
fn refs(values: &[String], known: &BTreeSet<String>) -> Result<(), String> {
    let seen = ids(values.iter().map(String::as_str))?;
    if let Some(unknown) = seen.iter().find(|id| !known.contains(*id)) {
        return Err(format!("Dangling reference: {unknown}"));
    }
    Ok(())
}
fn strings<'a>(values: impl Iterator<Item = &'a str>) -> Result<(), String> {
    if values.into_iter().all(nonempty) {
        Ok(())
    } else {
        Err("Declared text must be nonempty, at most 8192 bytes and contain no NUL".into())
    }
}

pub(super) fn validate(case: &Case) -> Result<(), String> {
    if case.version != 1 {
        return Err("Framing case version must be 1".into());
    }
    if case.stages.len() > 32
        || case.links.len() > 128
        || case.evidence.len() > 64
        || case.hypotheses.len() > 32
        || case.attempts.len() > 64
        || case.configuration.len() > 64
        || case.unknowns.len() > 64
    {
        return Err("Framing case exceeds collection limits".into());
    }
    strings(
        [
            case.revision.as_str(),
            &case.run_id,
            &case.input_digest,
            &case.scope,
            &case.problem.expected,
            &case.problem.observed,
            &case.problem.reproduction,
        ]
        .into_iter(),
    )?;
    if case.input_digest.len() != 64 || !case.input_digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("input_digest must be a 64-character SHA-256 value".into());
    }
    let stages = ids(case.stages.iter().map(|value| value.id.as_str()))?;
    let evidence = ids(case.evidence.iter().map(|value| value.id.as_str()))?;
    let hypotheses = ids(case.hypotheses.iter().map(|value| value.id.as_str()))?;
    ids(case.attempts.iter().map(|value| value.id.as_str()))?;
    ids(case
        .hypotheses
        .iter()
        .flat_map(|value| value.experiments.iter())
        .map(|value| value.id.as_str()))?;
    ids(case
        .stages
        .iter()
        .map(|item| item.id.as_str())
        .chain(case.evidence.iter().map(|item| item.id.as_str()))
        .chain(case.hypotheses.iter().map(|item| item.id.as_str()))
        .chain(case.attempts.iter().map(|item| item.id.as_str()))
        .chain(
            case.hypotheses
                .iter()
                .flat_map(|item| item.experiments.iter())
                .map(|item| item.id.as_str()),
        ))?;
    if !stages.contains(&case.entry_stage) || !stages.contains(&case.symptom_stage) {
        return Err("Entry and symptom must reference declared stages".into());
    }
    refs(&case.problem.failure_observations, &evidence)?;
    strings(case.unknowns.iter().map(String::as_str))?;
    for stage in &case.stages {
        strings([stage.purpose.as_str()].into_iter())?;
        if let Some(owner) = &stage.owner {
            strings([owner.as_str()].into_iter())?;
        }
        if let Some(reason) = &stage.schema_not_applicable {
            strings([reason.as_str()].into_iter())?;
        }
        refs(&stage.sources, &evidence)?;
        refs(&stage.schemas, &evidence)?;
        refs(&stage.enforcement_observations, &evidence)?;
    }
    let mut links = BTreeSet::new();
    for link in &case.links {
        if !stages.contains(&link.from)
            || !stages.contains(&link.to)
            || !links.insert((&link.from, &link.to))
        {
            return Err("Links must have distinct declared stage endpoints".into());
        }
        refs(&link.observations, &evidence)?;
    }
    for item in &case.evidence {
        if !evidence::relative(&item.path)
            || item.sha256.len() != 64
            || !item
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(format!(
                "Invalid relative evidence path or lowercase SHA-256: {}",
                item.id
            ));
        }
        if item.assertions.len() > 64 {
            return Err("At most 64 assertions per observation".into());
        }
        if item.kind != EvidenceKind::Observation && !item.assertions.is_empty() {
            return Err(
                "Source/schema evidence cannot substitute for observation assertions".into(),
            );
        }
        let mut pointers = BTreeSet::new();
        for assertion in &item.assertions {
            if assertion.pointer.len() > 1024
                || !(assertion.pointer == "/data" || assertion.pointer.starts_with("/data/"))
                || !pointers.insert(&assertion.pointer)
            {
                return Err("Assertions require unique /data JSON pointers".into());
            }
        }
    }
    let mut keys = BTreeSet::new();
    for config in &case.configuration {
        if !stages.contains(&config.owner_stage) {
            return Err("Configuration owner_stage must reference a declared stage".into());
        }
        refs(&config.consumers, &stages)?;
        strings([config.key.as_str()].into_iter())?;
        if !keys.insert(&config.key) || config.sources.len() > 16 {
            return Err("Configuration keys must be unique, at most 16 ordered sources".into());
        }
        let mut names = BTreeSet::new();
        for source in &config.sources {
            strings([source.name.as_str()].into_iter())?;
            if !names.insert(&source.name)
                || !evidence.contains(&source.evidence)
                || source.pointer.len() > 1024
                || !(source.pointer == "/data" || source.pointer.starts_with("/data/"))
            {
                return Err("Configuration sources need unique names, observation references and /data pointers".into());
            }
        }
    }
    for hypothesis in &case.hypotheses {
        strings([hypothesis.claim.as_str()].into_iter())?;
        refs(&hypothesis.stages, &stages)?;
        refs(&hypothesis.support, &evidence)?;
        refs(&hypothesis.counterevidence, &evidence)?;
        if hypothesis.experiments.len() > 16 {
            return Err("At most 16 experiments per hypothesis".into());
        }
        for experiment in &hypothesis.experiments {
            strings(
                [
                    experiment.prediction.as_str(),
                    &experiment.alternative_prediction,
                ]
                .into_iter(),
            )?;
            refs(&experiment.observations, &evidence)?;
        }
    }
    for attempt in &case.attempts {
        strings([attempt.summary.as_str()].into_iter())?;
        if !hypotheses.contains(&attempt.hypothesis) {
            return Err("Attempt references unknown hypothesis".into());
        }
        refs(&attempt.observations, &evidence)?;
    }
    if let Some(patch) = &case.proposed_patch {
        strings([patch.rationale.as_str()].into_iter())?;
        if !hypotheses.contains(&patch.hypothesis)
            || patch.paths.len() > 32
            || patch.max_changed_lines > 10000
        {
            return Err(
                "Patch must name a declared hypothesis and bounded paths/line budget".into(),
            );
        }
        let mut paths = BTreeSet::new();
        if patch
            .paths
            .iter()
            .any(|path| !evidence::relative(path) || !paths.insert(path))
        {
            return Err("Patch paths must be distinct repository-relative paths".into());
        }
    }
    Ok(())
}

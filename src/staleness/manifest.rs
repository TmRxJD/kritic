use std::collections::BTreeSet;
use std::path::Path;

use super::{Manifest, files};

pub(super) fn load(base: &Path, path: &str) -> Result<(Manifest, String), String> {
    let mut budget = files::MAX_TOTAL;
    let bytes = files::read(base, path, &mut budget)?;
    let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
    let manifest: Manifest =
        toml::from_str(text).map_err(|e| format!("Invalid test contract manifest: {e}"))?;
    validate(&manifest)?;
    super::invariants::validate(&manifest)?;
    if manifest
        .generated
        .iter()
        .any(|g| g.path.eq_ignore_ascii_case(path))
    {
        return Err("Generated fixture cannot overwrite the manifest".into());
    }
    Ok((manifest, crate::contract::hash(&bytes)))
}

fn nonempty(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 4096
}
fn digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
fn unique(set: &mut BTreeSet<String>, value: &str, kind: &str) -> Result<(), String> {
    if !nonempty(value) || !set.insert(value.to_lowercase()) {
        return Err(format!("{kind} must be nonempty and unique: {value}"));
    }
    Ok(())
}

pub(super) fn validate(m: &Manifest) -> Result<(), String> {
    if m.version != 1
        || m.test_roots.is_empty()
        || m.test_roots.len() > 16
        || m.authorities.is_empty()
        || m.authorities.len() > 100
        || m.tests.is_empty()
        || m.tests.len() > 1000
        || m.generated.len() > 100
        || m.exclusions.len() > 100
    {
        return Err("Require version=1, 1..16 roots, 1..100 authorities, 1..1000 tests, at most 100 fixtures/exclusions".into());
    }
    if m.test_pattern.len() > 4096 {
        return Err("Test regex exceeds 4096 bytes".into());
    }
    let re = regex::Regex::new(&m.test_pattern).map_err(|e| e.to_string())?;
    if re.is_match("") {
        return Err("Test pattern cannot match empty input".into());
    }
    for root in &m.test_roots {
        files::relative(root)?;
    }
    let mut ids = BTreeSet::new();
    let mut domains = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for a in &m.authorities {
        unique(&mut ids, &a.id, "Authority ID")?;
        unique(&mut domains, &a.domain, "Canonical domain")?;
        unique(&mut paths, &a.path, "Authority path")?;
        files::relative(&a.path)?;
        if !nonempty(&a.owner) || !nonempty(&a.purpose) {
            return Err("Authorities need owner and purpose".into());
        }
    }
    let mut tests = BTreeSet::new();
    for test in &m.tests {
        files::relative(&test.path)?;
        unique(&mut tests, &test.path, "Test path")?;
        if !nonempty(&test.purpose)
            || !nonempty(&test.validation_command)
            || !digest(&test.reviewed_test_sha256)
            || test.reviewed_inputs.is_empty()
            || test.reviewed_inputs.len() > 100
        {
            return Err(format!(
                "Test {} needs purpose, command, SHA-256 and 1..100 reviewed inputs",
                test.path
            ));
        }
        for (id, hash) in &test.reviewed_inputs {
            if !m.authorities.iter().any(|a| &a.id == id) || !digest(hash) {
                return Err(format!(
                    "Unknown authority or invalid reviewed SHA-256: {id}"
                ));
            }
        }
        if paths.contains(&test.path.to_lowercase()) {
            return Err("Test implementation cannot be its own canonical authority".into());
        }
    }
    let mut generated = BTreeSet::new();
    for fixture in &m.generated {
        files::relative(&fixture.path)?;
        unique(&mut generated, &fixture.path, "Generated path")?;
        if paths.contains(&fixture.path.to_lowercase())
            || tests.contains(&fixture.path.to_lowercase())
            || re.is_match(&fixture.path)
        {
            return Err(
                "Generated fixtures cannot overwrite tests or canonical authorities".into(),
            );
        }
        if !nonempty(&fixture.purpose)
            || (!fixture.pointer.is_empty() && !fixture.pointer.starts_with('/'))
            || fixture.pointer.len() > 4096
            || fixture.consumers.is_empty()
            || fixture.consumers.len() > 1000
        {
            return Err(
                "Generated fixture needs purpose, valid JSON Pointer and bounded consumers".into(),
            );
        }
        if !m.authorities.iter().any(|a| a.id == fixture.authority) {
            return Err("Unknown generated fixture authority".into());
        }
        for consumer in &fixture.consumers {
            if !m
                .tests
                .iter()
                .any(|t| &t.path == consumer && t.reviewed_inputs.contains_key(&fixture.authority))
            {
                return Err(format!("Fixture consumer must bind authority: {consumer}"));
            }
        }
    }
    let mut exclusions = BTreeSet::new();
    for e in &m.exclusions {
        files::relative(&e.path)?;
        unique(&mut exclusions, &e.path, "Excluded test path")?;
        if !nonempty(&e.reason) || tests.contains(&e.path.to_lowercase()) {
            return Err("Exclusions need a reason and cannot also be mapped tests".into());
        }
    }
    for left in &m.generated {
        for right in &m.generated {
            if left.path != right.path
                && right
                    .path
                    .to_lowercase()
                    .starts_with(&format!("{}/", left.path.to_lowercase()))
            {
                return Err("Generated destinations cannot be parents of each other".into());
            }
        }
    }
    Ok(())
}

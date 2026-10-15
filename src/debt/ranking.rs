use super::{DebtItem, Dependency, Effort, ReleaseInfo};
use std::cmp::Ordering;

/// A conservative stable-numeric subset; qualified/prerelease ecosystems remain unordered.
/// Never use lexical ordering to turn a newer installed version into a downgrade suggestion.
fn numeric(value: &str) -> Option<Vec<u64>> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let parts: Vec<_> = value.split('.').collect();
    if parts.is_empty() || parts.len() > 8 {
        return None;
    }
    parts
        .into_iter()
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        })
        .collect()
}

pub(super) fn estimate(rating: Option<u8>, label: &str) -> Effort {
    Effort { rating, minimum_rating: rating.map(|_| 1), maximum_rating: rating.map(|r| if r >= 4 { 5 } else { 3 }),
        estimated: true, label: label.into(), evidence: Vec::new(),
        possibilities: vec![
            "A version bump and lockfile regeneration may be sufficient; no successful update has been attempted.".into(),
            "Configuration, runtime, peer dependencies or new environment-variable names may need changes; verify migration notes instead of assuming them.".into(),
            "Removed APIs or changed behavior can require a refactor even within a nominally compatible release.".into(),
        ], required_validation: vec!["Review upstream release/migration notes and project constraints".into(),
            "Review the resolved dependency/lockfile diff and runtime/peer requirements".into(),
            "Run the project's meaningful build, typecheck, tests and affected user flows".into()] }
}

pub fn rank_dependency(dependency: &Dependency, release: &ReleaseInfo) -> DebtItem {
    let mut urgency = "unknown";
    let mut reason =
        "Installed/latest version evidence is incomplete; no upgrade conclusion".to_string();
    let mut status = "unknown";
    let mut effort = estimate(None, "Migration effort unverified");
    if let (Some(current), Some(latest)) = (&dependency.current, &release.latest) {
        if current == latest {
            status = "matches_observed_default";
            urgency = "info";
            reason = "Resolved version matches the observed package default; this does not prove absence of debt or vulnerabilities".into();
        } else if let (Some(mut current), Some(mut latest)) = (numeric(current), numeric(latest)) {
            let length = current.len().max(latest.len());
            current.resize(length, 0);
            latest.resize(length, 0);
            match current.cmp(&latest) {
                Ordering::Greater => {
                    status = "ahead_of_observed_default";
                    urgency = "info";
                    reason = "Installed version is newer than the observed default; do not automatically downgrade".into();
                }
                Ordering::Equal => {
                    status = "equivalent_numeric_version";
                    urgency = "info";
                    reason = "Stable numeric versions compare equal; ecosystem-specific constraints still require review".into();
                }
                Ordering::Less => {
                    status = if release.status == "stale_cache" {
                        "cached_update_candidate"
                    } else {
                        "update_candidate"
                    };
                    let major = current[0] != latest[0];
                    let pre_one_break = current[0] == 0 && current.get(1) != latest.get(1);
                    if major || pre_one_break {
                        urgency = "medium";
                        reason = "A newer major/pre-1.0 minor default is available; schedule compatibility review, not an emergency solely because it is newer".into();
                        effort = estimate(Some(4), "Potential breaking API/refactor investigation");
                    } else if current.get(1) != latest.get(1) {
                        urgency = "low";
                        reason = "A newer stable minor default is available; routine maintenance candidate".into();
                        effort = estimate(Some(2), "Dependency/lockfile update candidate");
                    } else {
                        urgency = "low";
                        reason = "A newer stable patch default is available; routine maintenance candidate".into();
                        effort = estimate(
                            Some(1),
                            "Small version-bump candidate; not verified one-click",
                        );
                    }
                }
            }
        } else {
            status = "different_versions_unordered";
            reason = "Versions differ but include ecosystem qualifiers or unsupported ordering; inspect the observed versions without assuming upgrade direction".into();
        }
    } else if dependency.current.is_none() {
        status = "installed_version_unknown";
    }
    if !release.advisory_ids.is_empty() {
        urgency = "high";
        reason = format!(
            "Advisory identifiers reported for the resolved version: {}. Prioritize security triage; severity/exploitability and fixed versions are not established here",
            release.advisory_ids.join(", ")
        );
    }
    if release.current_deprecated == Some(true) {
        urgency = "high";
        reason
            .push_str(" The current release is marked deprecated; plan migration or replacement.");
    }
    if release.latest_deprecated == Some(true) {
        reason.push_str(
            " The target/default is also marked deprecated; do not recommend it blindly.",
        );
        effort = estimate(
            None,
            "Select a supported target before estimating migration",
        );
    }
    if release.status == "stale_cache" {
        reason.push_str(" This is stale cached evidence; refresh before deciding.");
    }
    if !release.errors.is_empty() {
        reason.push_str(" Lookup coverage is partial; inspect the reported errors.");
    }
    if let Some(runtime) = &release.requires_runtime {
        effort.evidence.push(format!("Latest release declares runtime requirements: {runtime}; compare with the project's actual runtime"));
    }
    if !release.peer_dependencies.is_empty() {
        effort.evidence.push(format!(
            "Latest release declares {} peer requirements; compatibility has not been tested",
            release.peer_dependencies.len()
        ));
    }
    if dependency.scope == "transitive" {
        effort.required_validation.push("Identify the owning direct dependency; prefer its supported upgrade path over forcing a transitive lockfile edit".into());
    }
    DebtItem {
        id: super::identity(&format!(
            "{}:{}:{:?}:{}",
            dependency.ecosystem.as_str(),
            dependency.name,
            dependency.current,
            dependency.manifest
        )),
        category: "dependency".into(),
        summary: format!(
            "{} ({}) maintenance review",
            dependency.name,
            dependency.ecosystem.as_str()
        ),
        urgency: urgency.into(),
        urgency_reason: reason,
        ecosystem: Some(dependency.ecosystem),
        package: Some(dependency.name.clone()),
        current: dependency.current.clone(),
        latest: release.latest.clone(),
        version_status: status.into(),
        source: dependency.manifest.clone(),
        source_line: None,
        effort,
        lookup: Some(release.clone()),
    }
}

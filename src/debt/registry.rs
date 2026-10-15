//! Explicit public-registry observations, not upgrade safety or current-version approval.
mod cache;
mod http;
mod parse;
#[cfg(test)]
mod tests;

use super::{Dependency, Ecosystem, ReleaseInfo};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    latest: Option<String>,
    source_url: Option<String>,
    metadata_sources: Vec<String>,
    current_deprecated: Option<bool>,
    latest_deprecated: Option<bool>,
    deprecation_reason: Option<String>,
    requires_runtime: Option<String>,
    peer_dependencies: BTreeMap<String, String>,
    advisory_ids: Vec<String>,
    errors: Vec<String>,
}

fn report(
    dependency: &Dependency,
    data: Metadata,
    status: &str,
    observed_at: Option<String>,
    age: Option<i64>,
) -> ReleaseInfo {
    ReleaseInfo {
        ecosystem: dependency.ecosystem,
        name: dependency.name.clone(),
        current: dependency.current.clone(),
        latest: data.latest,
        source_url: data.source_url,
        metadata_sources: data.metadata_sources,
        observed_at,
        cache_age_seconds: age,
        status: status.into(),
        current_deprecated: data.current_deprecated,
        latest_deprecated: data.latest_deprecated,
        deprecation_reason: data.deprecation_reason,
        requires_runtime: data.requires_runtime,
        peer_dependencies: data.peer_dependencies,
        advisory_ids: data.advisory_ids,
        errors: data.errors,
    }
}

fn identity(dependency: &Dependency) -> String {
    // JSON tuples avoid delimiter collisions in scoped names and version strings.
    serde_json::to_string(&(
        1,
        dependency.ecosystem.as_str(),
        &dependency.name,
        &dependency.current,
    ))
    .expect("string tuple serialization")
}

fn valid(dependency: &Dependency) -> bool {
    !dependency.name.is_empty()
        && dependency.name.len() <= 512
        && !dependency.name.chars().any(char::is_control)
        && dependency.current.as_ref().is_none_or(|version| {
            !version.is_empty() && version.len() <= 256 && !version.chars().any(char::is_control)
        })
}

/// One request plan per exact package/current/public identity; manifests and scopes
/// share registry metadata, but private declarations never join public requests.
fn distinct_indices(dependencies: &[Dependency]) -> Vec<Vec<usize>> {
    let mut grouped: BTreeMap<(String, bool), Vec<usize>> = BTreeMap::new();
    for (index, dependency) in dependencies.iter().enumerate() {
        grouped
            .entry((identity(dependency), dependency.public_registry))
            .or_default()
            .push(index);
    }
    grouped.into_values().collect()
}

async fn within_deadline(
    dependency: &Dependency,
    deadline: Option<tokio::time::Instant>,
    work: impl std::future::Future<Output = ReleaseInfo>,
) -> ReleaseInfo {
    let exhausted = || {
        report(dependency, Metadata {
        errors: vec!["Online registry batch exhausted its 60-second deadline; lookup coverage is incomplete".into()],
        ..Default::default()
    }, "budget_exhausted", None, None)
    };
    let Some(deadline) = deadline else {
        return work.await;
    };
    // Do not poll queued work after expiry: it could otherwise start another request.
    if deadline <= tokio::time::Instant::now() {
        return exhausted();
    }
    match tokio::time::timeout_at(deadline, work).await {
        Ok(result) => result,
        Err(_) => exhausted(),
    }
}

/// Only online=true permits public metadata requests. Fresh cache avoids them unless
/// refresh=true; stale offline metadata is explicitly labelled and never an up-to-date claim.
pub async fn lookup_all(
    dependencies: &[Dependency],
    cache: &Path,
    online: bool,
    refresh: bool,
) -> Vec<ReleaseInfo> {
    let deadline = online.then(|| tokio::time::Instant::now() + std::time::Duration::from_secs(60));
    let client = http::client();
    let results = stream::iter(distinct_indices(dependencies))
        .map(|indices| {
            let client = &client;
            async move {
                let dependency = &dependencies[indices[0]];
                (
                    indices,
                    within_deadline(
                        dependency,
                        deadline,
                        lookup(
                            dependency,
                            cache,
                            online,
                            refresh,
                            client.as_ref().map_err(String::as_str),
                        ),
                    )
                    .await,
                )
            }
        })
        .buffer_unordered(6)
        .collect::<Vec<_>>()
        .await;
    let mut ordered = BTreeMap::new();
    for (indices, result) in results {
        for index in indices {
            ordered.insert(index, result.clone());
        }
    }
    ordered.into_values().collect()
}

async fn lookup(
    dependency: &Dependency,
    cache_root: &Path,
    online: bool,
    refresh: bool,
    client: Result<&reqwest::Client, &str>,
) -> ReleaseInfo {
    let mut empty = Metadata::default();
    if !dependency.public_registry {
        empty
            .errors
            .push("Private/custom registry dependency was not sent to a public registry".into());
        return report(dependency, empty, "nonpublic_skipped", None, None);
    }
    if !valid(dependency) {
        empty
            .errors
            .push("Invalid or oversized package name/current version".into());
        return report(dependency, empty, "error", None, None);
    }
    let identity = identity(dependency);
    let cached = match cache::read(cache_root, &identity) {
        Ok(value) => value,
        Err(error) => {
            empty.errors.push(error);
            None
        }
    };
    if let Some((data, observed, age)) = &cached
        && (!online || (!refresh && *age < 86400))
    {
        let mut data = data.clone();
        data.errors.extend(empty.errors);
        return report(
            dependency,
            data,
            if *age < 86400 {
                "fresh_cache"
            } else {
                "stale_cache"
            },
            Some(observed.clone()),
            Some(*age),
        );
    }
    if !online {
        empty
            .errors
            .push("Public registry lookup disabled; no usable cached metadata".into());
        return report(dependency, empty, "offline_missing", None, None);
    }
    let client = match client {
        Ok(client) => client,
        Err(error) => {
            empty.errors.push(error.into());
            return report(dependency, empty, "error", None, None);
        }
    };
    let mut data = fetch(dependency, client).await;
    data.errors.extend(empty.errors);
    let observed = chrono::Utc::now().to_rfc3339();
    if data.latest.is_some()
        && let Err(error) = cache::write(cache_root, &identity, &data, &observed)
    {
        data.errors.push(error);
    }
    let status = if data.errors.is_empty() {
        "fetched"
    } else if data.latest.is_some() {
        "partial"
    } else {
        "error"
    };
    // Failed refreshes preserve old observations explicitly, rather than replacing them
    // with an empty success or presenting old data as freshly fetched.
    if data.latest.is_none()
        && let Some((mut old, timestamp, age)) = cached
    {
        old.errors.extend(data.errors);
        return report(dependency, old, "stale_cache", Some(timestamp), Some(age));
    }
    report(dependency, data, status, Some(observed), Some(0))
}

fn request_name(dependency: &Dependency) -> String {
    http::segment(&parse::normalize(dependency.ecosystem, &dependency.name))
}

fn latest_url(dependency: &Dependency) -> String {
    let name = request_name(dependency);
    match dependency.ecosystem {
        Ecosystem::Cargo => format!("https://crates.io/api/v1/crates/{name}"),
        Ecosystem::Npm => format!("https://registry.npmjs.org/{name}/latest"),
        Ecosystem::Pypi => format!("https://pypi.org/pypi/{name}/json"),
        system => format!(
            "https://api.deps.dev/v3/systems/{}/packages/{name}",
            system.system()
        ),
    }
}

async fn fetch(dependency: &Dependency, client: &reqwest::Client) -> Metadata {
    let mut data = Metadata::default();
    let name = request_name(dependency);
    let latest_url = latest_url(dependency);
    data.source_url = Some(latest_url.clone());
    data.metadata_sources.push(latest_url.clone());
    match http::get(client, &latest_url).await {
        Ok(value) => {
            if let Err(error) = parse::latest(dependency, &value, &mut data) {
                data.errors.push(error);
            }
        }
        Err(error) => data.errors.push(format!("Latest metadata: {error}")),
    }
    if let Some(version) = &dependency.current {
        if dependency.ecosystem == Ecosystem::Npm {
            let url = format!(
                "https://registry.npmjs.org/{name}/{}",
                http::segment(version)
            );
            data.metadata_sources.push(url.clone());
            match http::get(client, &url).await {
                Ok(value) => {
                    if let Err(error) = parse::npm_current(dependency, &value, &mut data) {
                        data.errors.push(error);
                    }
                }
                Err(error) => data.errors.push(format!("Current npm metadata: {error}")),
            }
        }
        let url = format!(
            "https://api.deps.dev/v3/systems/{}/packages/{name}/versions/{}",
            dependency.ecosystem.system(),
            http::segment(version)
        );
        data.metadata_sources.push(url.clone());
        match http::get(client, &url).await {
            Ok(value) => {
                if let Err(error) = parse::current(dependency, &value, &mut data) {
                    data.errors.push(error);
                }
            }
            Err(error) => data
                .errors
                .push(format!("Current advisory metadata: {error}")),
        }
    } else {
        data.errors.push(
            "Current version unknown; current deprecation/advisory coverage unavailable".into(),
        );
    }
    data
}

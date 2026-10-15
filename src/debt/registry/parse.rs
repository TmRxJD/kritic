//! Registry JSON is untrusted evidence; response package/version identities must agree.
use super::{Dependency, Ecosystem, Metadata};
use serde_json::Value;

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
        .map(str::to_owned)
}
fn normalize(ecosystem: Ecosystem, name: &str) -> String {
    match ecosystem {
        Ecosystem::Pypi => {
            let mut normalized = String::new();
            for ch in name.to_ascii_lowercase().chars() {
                if matches!(ch, '-' | '_' | '.') {
                    if !normalized.ends_with('-') {
                        normalized.push('-');
                    }
                } else {
                    normalized.push(ch);
                }
            }
            normalized
        }
        Ecosystem::Nuget => name.to_ascii_lowercase(),
        _ => name.to_owned(),
    }
}
fn same_name(dependency: &Dependency, value: &Value) -> bool {
    text(value).is_some_and(|name| {
        normalize(dependency.ecosystem, &name) == normalize(dependency.ecosystem, &dependency.name)
    })
}
fn identity(dependency: &Dependency, key: &Value, current: bool) -> bool {
    same_name(dependency, &key["name"])
        && key["system"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(dependency.ecosystem.system()))
        && (!current || key["version"].as_str() == dependency.current.as_deref())
}
fn deprecation(value: &Value) -> Result<(bool, Option<String>), String> {
    if !value["deprecated"].is_null() && !value["deprecated"].is_string() {
        return Err("Malformed npm deprecation metadata".into());
    }
    let reason = text(&value["deprecated"]);
    if value["deprecated"]
        .as_str()
        .is_some_and(|value| !value.is_empty())
        && reason.is_none()
    {
        return Err("Invalid or oversized npm deprecation reason".into());
    }
    Ok((reason.is_some(), reason))
}
fn deprecated(value: &Value) -> Result<bool, String> {
    if value["isDeprecated"].is_null() {
        return Ok(false);
    }
    value["isDeprecated"]
        .as_bool()
        .ok_or_else(|| "Malformed deps.dev deprecation flag".into())
}

pub(super) fn latest(
    dependency: &Dependency,
    value: &Value,
    metadata: &mut Metadata,
) -> Result<(), String> {
    match dependency.ecosystem {
        Ecosystem::Cargo => {
            let package = &value["crate"];
            if !same_name(dependency, &package["id"]) {
                return Err("Cargo response package identity mismatch".into());
            }
            metadata.latest = text(&package["max_stable_version"]);
            if let Some(versions) = value["versions"].as_array()
                && let Some(version) = versions
                    .iter()
                    .find(|v| v["num"].as_str() == metadata.latest.as_deref())
            {
                metadata.requires_runtime =
                    text(&version["rust_version"]).map(|v| format!("rust={v}"));
            }
        }
        Ecosystem::Npm => {
            if !same_name(dependency, &value["name"]) {
                return Err("npm response package identity mismatch".into());
            }
            metadata.latest = text(&value["version"]);
            let (deprecated, reason) = deprecation(value)?;
            metadata.latest_deprecated = Some(deprecated);
            metadata.deprecation_reason = reason;
            if let Some(engines) = value["engines"].as_object() {
                if engines.len() > 64 {
                    return Err("npm engines metadata exceeds limit".into());
                }
                let mut requirements = vec![];
                for (name, value) in engines {
                    let Some(requirement) = text(value) else {
                        return Err("Invalid npm engines metadata".into());
                    };
                    if name.len() > 128 {
                        return Err("npm engine name exceeds limit".into());
                    }
                    requirements.push(format!("{name}={requirement}"));
                }
                metadata.requires_runtime = Some(requirements.join("; "));
            }
            if let Some(peers) = value["peerDependencies"].as_object() {
                if peers.len() > 256 {
                    return Err("npm peer metadata exceeds limit".into());
                }
                for (name, requirement) in peers {
                    if name.len() > 512 {
                        return Err("npm peer name exceeds limit".into());
                    }
                    metadata.peer_dependencies.insert(
                        name.clone(),
                        text(requirement).ok_or("Invalid npm peer metadata")?,
                    );
                }
            }
        }
        Ecosystem::Pypi => {
            if !same_name(dependency, &value["info"]["name"]) {
                return Err("PyPI response package identity mismatch".into());
            }
            metadata.latest = text(&value["info"]["version"]);
            metadata.requires_runtime =
                text(&value["info"]["requires_python"]).map(|v| format!("python={v}"));
        }
        _ => {
            if !identity(dependency, &value["packageKey"], false) {
                return Err("deps.dev package identity mismatch".into());
            }
            let versions = value["versions"]
                .as_array()
                .ok_or("deps.dev versions list missing")?;
            let defaults: Vec<_> = versions.iter().filter(|v| v["isDefault"] == true).collect();
            if defaults.len() != 1 {
                return Err(
                    "deps.dev has no unique declared default version; latest remains unknown"
                        .into(),
                );
            }
            let default = defaults[0];
            if !identity(dependency, &default["versionKey"], false) {
                return Err("deps.dev default-version identity mismatch".into());
            }
            metadata.latest = text(&default["versionKey"]["version"]);
            metadata.latest_deprecated = Some(deprecated(default)?);
            metadata.deprecation_reason = text(&default["deprecatedReason"]);
        }
    }
    if metadata
        .latest
        .as_ref()
        .is_none_or(|version| version.len() > 256)
    {
        metadata.latest = None;
        return Err("Registry did not provide a bounded latest stable/default version".into());
    }
    Ok(())
}

pub(super) fn npm_current(
    dependency: &Dependency,
    value: &Value,
    metadata: &mut Metadata,
) -> Result<(), String> {
    if !same_name(dependency, &value["name"])
        || value["version"].as_str() != dependency.current.as_deref()
    {
        return Err("Current npm response package/version identity mismatch".into());
    }
    let (deprecated, reason) = deprecation(value)?;
    metadata.current_deprecated = Some(deprecated);
    if reason.is_some() {
        metadata.deprecation_reason = reason;
    }
    Ok(())
}

pub(super) fn current(
    dependency: &Dependency,
    value: &Value,
    metadata: &mut Metadata,
) -> Result<(), String> {
    if !identity(dependency, &value["versionKey"], true) {
        return Err("deps.dev current package/version identity mismatch".into());
    }
    metadata.current_deprecated =
        Some(metadata.current_deprecated.unwrap_or(false) || deprecated(value)?);
    if let Some(reason) = text(&value["deprecatedReason"]) {
        metadata.deprecation_reason = Some(reason);
    }
    if !value["advisoryKeys"].is_null() && !value["advisoryKeys"].is_array() {
        return Err("Malformed deps.dev advisory list".into());
    }
    if let Some(advisories) = value["advisoryKeys"].as_array() {
        if advisories.len() > 256 {
            return Err("Advisory list exceeds 256 entries".into());
        }
        for advisory in advisories {
            let id = text(&advisory["id"])
                .filter(|id| id.len() <= 256)
                .ok_or("Malformed advisory identifier")?;
            metadata.advisory_ids.push(id);
        }
        metadata.advisory_ids.sort();
        metadata.advisory_ids.dedup();
    }
    Ok(())
}

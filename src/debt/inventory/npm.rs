use super::{Dependency, Ecosystem, Inventory, Locked, add, exact, parent};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn registry_public(
    name: &str,
    manifest: &str,
    files: &BTreeMap<String, String>,
) -> bool {
    let mut candidates: Vec<_> = files
        .iter()
        .filter(|(path, _)| {
            path.ends_with(".npmrc")
                && (parent(path).is_empty()
                    || parent(manifest) == parent(path)
                    || parent(manifest).starts_with(&format!("{}/", parent(path))))
        })
        .collect();
    candidates.sort_by_key(|(path, _)| parent(path).len());
    let scope = name
        .strip_prefix('@')
        .and_then(|s| s.split_once('/'))
        .map(|(s, _)| format!("@{s}:registry"));
    let mut public = true;
    let mut scoped = None;
    for (_, text) in candidates {
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with(['#', ';']))
        {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "registry" {
                    public = public_url(value.trim());
                } else if scope.as_deref() == Some(key.trim()) {
                    scoped = Some(public_url(value.trim()));
                }
            }
        }
    }
    scoped.unwrap_or(public)
}

fn public_url(value: &str) -> bool {
    reqwest::Url::parse(value).ok().is_some_and(|url| {
        matches!(
            url.host_str(),
            Some("registry.npmjs.org" | "registry.yarnpkg.com")
        ) && url.username().is_empty()
            && url.password().is_none()
    })
}

pub(super) fn lock(
    name: &str,
    text: &str,
    files: &BTreeMap<String, String>,
    locked: &mut Vec<Locked>,
    inv: &mut Inventory,
) {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            inv.coverage_gaps
                .push(format!("Malformed npm lock {name:?}"));
            return;
        }
    };
    if let Some(packages) = value.get("packages").and_then(Value::as_object) {
        for (slot, package) in packages {
            if slot.is_empty() {
                continue;
            }
            let Some((consumer, guessed)) = slot.rsplit_once("node_modules/") else {
                if package.get("link").and_then(Value::as_bool) != Some(true) {
                    inv.coverage_gaps.push(format!("Workspace package entry in {name:?} needs its package.json for direct dependency inventory"));
                }
                continue;
            };
            let actual = package
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(guessed);
            insert(
                name,
                actual,
                package,
                Some(consumer.trim_end_matches('/').into()),
                files,
                locked,
                inv,
            );
        }
    } else if let Some(dependencies) = value.get("dependencies").and_then(Value::as_object) {
        for (pkg, item) in dependencies {
            insert(name, pkg, item, Some(String::new()), files, locked, inv);
            if item.get("dependencies").is_some() {
                inv.coverage_gaps.push(format!("Nested npm v1 lock graph in {name:?} is only partially inventoried; use a v2/v3 lock"));
            }
        }
    } else {
        inv.coverage_gaps.push(format!(
            "npm lock {name:?} lacks recognized package records"
        ));
    }
}

fn insert(
    manifest: &str,
    name: &str,
    item: &Value,
    consumer: Option<String>,
    files: &BTreeMap<String, String>,
    locked: &mut Vec<Locked>,
    inv: &mut Inventory,
) {
    let Some(version) = item
        .get("version")
        .and_then(Value::as_str)
        .filter(|v| exact(v))
    else {
        inv.coverage_gaps.push(format!(
            "Unresolved/nonexact npm lock entry {name:?} in {manifest:?}"
        ));
        return;
    };
    let public = item.get("link").and_then(Value::as_bool) != Some(true)
        && item
            .get("resolved")
            .and_then(Value::as_str)
            .is_some_and(public_url)
        && registry_public(name, manifest, files);
    super::push_locked(
        locked,
        Locked {
            name: name.into(),
            version: version.into(),
            manifest: manifest.into(),
            public,
            consumer,
        },
    );
    super::push(
        inv,
        Dependency {
            ecosystem: Ecosystem::Npm,
            name: name.into(),
            requirement: None,
            current: Some(version.into()),
            scope: "transitive".into(),
            manifest: manifest.into(),
            public_registry: public,
        },
    );
}

pub(super) fn manifest(
    name: &str,
    text: &str,
    files: &BTreeMap<String, String>,
    locked: &[Locked],
    inv: &mut Inventory,
) {
    let value: Value = match serde_json::from_str(text) {
        Ok(value @ Value::Object(_)) => value,
        _ => {
            inv.coverage_gaps
                .push(format!("Malformed npm manifest {name:?}"));
            return;
        }
    };
    for section in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        let Some(entries) = value.get(section) else {
            continue;
        };
        let Some(entries) = entries.as_object() else {
            inv.coverage_gaps
                .push(format!("Invalid npm {section} in {name:?}"));
            continue;
        };
        for (alias, spec) in entries {
            let Some(spec) = spec.as_str() else {
                inv.coverage_gaps
                    .push(format!("Invalid npm requirement {alias:?} in {name:?}"));
                continue;
            };
            let (actual, requirement) = if let Some(target) = spec.strip_prefix("npm:") {
                match target.rsplit_once('@').filter(|(pkg, _)| !pkg.is_empty()) {
                    Some((pkg, range)) => (pkg, range),
                    None => (target, "*"),
                }
            } else {
                (alias.as_str(), spec)
            };
            let public = !requirement.contains(['/', ':'])
                && !matches!(requirement, "workspace:*" | "workspace:^")
                && registry_public(actual, name, files);
            let scoped: Vec<_> = locked
                .iter()
                .filter(|item| {
                    let relative = parent(name)
                        .strip_prefix(parent(&item.manifest))
                        .unwrap_or(parent(name))
                        .trim_start_matches('/');
                    let has_exact = locked.iter().any(|other| {
                        other.name == actual
                            && other.manifest == item.manifest
                            && other.consumer.as_deref() == Some(relative)
                    });
                    item.consumer.as_deref() == Some(if has_exact { relative } else { "" })
                })
                .cloned()
                .collect();
            add(
                inv,
                super::Declaration {
                    ecosystem: Ecosystem::Npm,
                    name: actual,
                    requirement: Some(requirement.into()),
                    scope: "direct",
                    manifest: name,
                    public,
                },
                &scoped,
            );
        }
    }
}

//! pnpm v9 importers bind direct versions to their actual workspace, including peers.
use super::{Dependency, Ecosystem, Inventory, Locked, exact, npm};
use serde_yaml::Value;
use std::collections::BTreeMap;

pub(super) fn lock(
    name: &str,
    text: &str,
    files: &BTreeMap<String, String>,
    locked: &mut Vec<Locked>,
    inv: &mut Inventory,
) {
    let value: Value = match serde_yaml::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            inv.coverage_gaps
                .push(format!("Malformed pnpm lock {name:?}"));
            return;
        }
    };
    let version = value.get("lockfileVersion").map(|value| {
        value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.as_f64().unwrap_or_default().to_string())
    });
    if !version.is_some_and(|version| version == "9" || version == "9.0") {
        inv.coverage_gaps.push(format!(
            "Unsupported pnpm lock version in {name:?}; only v9 is parsed"
        ));
        return;
    }
    if let Some(importers) = value.get("importers").and_then(Value::as_mapping) {
        for (importer, sections) in importers {
            let Some(importer) = importer.as_str() else {
                inv.coverage_gaps
                    .push(format!("Invalid pnpm importer in {name:?}"));
                continue;
            };
            let consumer = if importer == "." { "" } else { importer };
            for section in ["dependencies", "devDependencies", "optionalDependencies"] {
                let Some(deps) = sections.get(section).and_then(Value::as_mapping) else {
                    continue;
                };
                for (alias, entry) in deps {
                    let Some(alias) = alias.as_str() else {
                        inv.coverage_gaps
                            .push(format!("Invalid pnpm dependency key in {name:?}"));
                        continue;
                    };
                    let specification = entry.get("specifier").and_then(Value::as_str);
                    let actual = specification
                        .and_then(|spec| spec.strip_prefix("npm:"))
                        .map(|spec| {
                            spec.rsplit_once('@')
                                .filter(|(name, _)| !name.is_empty())
                                .map_or(spec, |(name, _)| name)
                        })
                        .unwrap_or(alias);
                    let raw = entry.get("version").and_then(Value::as_str).unwrap_or("");
                    let unpeered = raw.split('(').next().unwrap_or(raw);
                    let version = if exact(unpeered) {
                        Some(unpeered)
                    } else {
                        unpeered
                            .rsplit_once('@')
                            .map(|(_, version)| version)
                            .filter(|version| exact(version))
                    };
                    let direct_manifest = [super::parent(name), consumer, "package.json"]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join("/");
                    let public = version.is_some()
                        && !raw.contains(':')
                        && npm::registry_public(actual, &direct_manifest, files);
                    if let Some(version) = version {
                        super::push_locked(
                            locked,
                            Locked {
                                name: actual.into(),
                                version: version.into(),
                                manifest: name.into(),
                                public,
                                consumer: Some(consumer.into()),
                            },
                        );
                    }
                    super::push(
                        inv,
                        Dependency {
                            ecosystem: Ecosystem::Npm,
                            name: actual.into(),
                            requirement: specification.map(str::to_string),
                            current: version.map(str::to_string),
                            scope: "direct".into(),
                            manifest: format!("{name}#importers/{importer}"),
                            public_registry: public,
                        },
                    );
                    if raw.is_empty() {
                        inv.coverage_gaps.push(format!(
                            "Missing pnpm resolved version for {alias:?} in {name:?}"
                        ));
                    }
                }
            }
        }
    } else {
        inv.coverage_gaps
            .push(format!("pnpm v9 lock {name:?} lacks importer records"));
    }
    if let Some(packages) = value.get("packages").and_then(Value::as_mapping) {
        for (key, item) in packages {
            let Some(key) = key.as_str() else {
                inv.coverage_gaps
                    .push(format!("Invalid pnpm package key in {name:?}"));
                continue;
            };
            let key = key.split('(').next().unwrap_or(key);
            let Some((pkg, version)) = key
                .rsplit_once('@')
                .filter(|(pkg, version)| !pkg.is_empty() && exact(version))
            else {
                inv.coverage_gaps
                    .push(format!("Unresolved pnpm package record in {name:?}"));
                continue;
            };
            let public = item
                .get("resolution")
                .and_then(|v| v.get("tarball"))
                .is_none()
                && npm::registry_public(pkg, name, files);
            super::push(
                inv,
                Dependency {
                    ecosystem: Ecosystem::Npm,
                    name: pkg.into(),
                    requirement: None,
                    current: Some(version.into()),
                    scope: "transitive".into(),
                    manifest: name.into(),
                    public_registry: public,
                },
            );
        }
    }
}

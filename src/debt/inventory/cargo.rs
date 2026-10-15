use super::{Dependency, Ecosystem, Inventory, Locked, add, exact};

pub(super) fn lock(name: &str, text: &str, locked: &mut Vec<Locked>, inv: &mut Inventory) {
    let value: toml::Value = match toml::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            inv.coverage_gaps
                .push(format!("Malformed Cargo lock {name:?}"));
            return;
        }
    };
    let Some(packages) = value.get("package").and_then(toml::Value::as_array) else {
        inv.coverage_gaps
            .push(format!("Cargo lock {name:?} lacks package records"));
        return;
    };
    for package in packages {
        let Some(pkg) = package.get("name").and_then(toml::Value::as_str) else {
            inv.coverage_gaps
                .push(format!("Invalid package record in {name:?}"));
            continue;
        };
        let Some(version) = package
            .get("version")
            .and_then(toml::Value::as_str)
            .filter(|v| exact(v))
        else {
            inv.coverage_gaps.push(format!(
                "Nonexact Cargo lock version for {pkg:?} in {name:?}"
            ));
            continue;
        };
        let public = package
            .get("source")
            .and_then(toml::Value::as_str)
            .is_some_and(|s| {
                matches!(
                    s.trim_end_matches('/'),
                    "registry+https://github.com/rust-lang/crates.io-index"
                        | "registry+https://index.crates.io"
                )
            });
        super::push_locked(
            locked,
            Locked {
                name: pkg.into(),
                version: version.into(),
                manifest: name.into(),
                public,
                consumer: None,
            },
        );
        super::push(
            inv,
            Dependency {
                ecosystem: Ecosystem::Cargo,
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

pub(super) fn manifest(name: &str, text: &str, locked: &[Locked], inv: &mut Inventory) {
    let value: toml::Value = match toml::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            inv.coverage_gaps
                .push(format!("Malformed Cargo manifest {name:?}"));
            return;
        }
    };
    visit(name, &value, locked, inv);
}

fn visit(manifest: &str, value: &toml::Value, locked: &[Locked], inv: &mut Inventory) {
    let Some(table) = value.as_table() else {
        return;
    };
    for (section, content) in table {
        if matches!(
            section.as_str(),
            "dependencies" | "dev-dependencies" | "build-dependencies"
        ) {
            let Some(dependencies) = content.as_table() else {
                inv.coverage_gaps
                    .push(format!("Invalid Cargo dependency section in {manifest:?}"));
                continue;
            };
            for (alias, spec) in dependencies {
                let actual = spec
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(alias);
                let requirement = spec
                    .as_str()
                    .or_else(|| spec.get("version").and_then(toml::Value::as_str))
                    .map(str::to_string);
                let public = spec.as_str().is_some()
                    || spec.as_table().is_some_and(|t| {
                        !t.contains_key("path")
                            && !t.contains_key("git")
                            && !t
                                .get("workspace")
                                .and_then(toml::Value::as_bool)
                                .unwrap_or(false)
                            && t.get("registry")
                                .and_then(toml::Value::as_str)
                                .is_none_or(|r| r == "crates-io")
                    });
                if !spec.is_str() && !spec.is_table() {
                    inv.coverage_gaps.push(format!(
                        "Invalid Cargo dependency {alias:?} in {manifest:?}"
                    ));
                    continue;
                }
                add(
                    inv,
                    super::Declaration {
                        ecosystem: Ecosystem::Cargo,
                        name: actual,
                        requirement,
                        scope: "direct",
                        manifest,
                        public,
                    },
                    locked,
                );
            }
        } else if content.is_table() {
            visit(manifest, content, locked, inv);
        }
    }
}

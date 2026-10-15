use super::{Dependency, Ecosystem, Inventory, Locked, add, exact};

pub(super) fn lock(name: &str, text: &str, locked: &mut Vec<Locked>, inv: &mut Inventory) {
    let value: toml::Value = match toml::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            inv.coverage_gaps
                .push(format!("Malformed Python lock {name:?}"));
            return;
        }
    };
    let Some(packages) = value.get("package").and_then(toml::Value::as_array) else {
        inv.coverage_gaps
            .push(format!("Python lock {name:?} lacks package records"));
        return;
    };
    for item in packages {
        let Some(pkg) = item.get("name").and_then(toml::Value::as_str) else {
            inv.coverage_gaps
                .push(format!("Invalid Python package record in {name:?}"));
            continue;
        };
        let Some(version) = item
            .get("version")
            .and_then(toml::Value::as_str)
            .filter(|v| exact(v))
        else {
            inv.coverage_gaps.push(format!(
                "Nonexact Python lock version for {pkg:?} in {name:?}"
            ));
            continue;
        };
        let public = match item.get("source") {
            None => !name.ends_with("uv.lock"),
            Some(source) => source
                .get("registry")
                .or_else(|| source.get("url"))
                .and_then(toml::Value::as_str)
                .is_some_and(|url| {
                    reqwest::Url::parse(url).ok().is_some_and(|url| {
                        url.host_str() == Some("pypi.org")
                            && url.username().is_empty()
                            && url.password().is_none()
                    })
                }),
        };
        let normalized = normalize(pkg);
        super::push_locked(
            locked,
            Locked {
                name: normalized.clone(),
                version: version.into(),
                manifest: name.into(),
                public,
                consumer: None,
            },
        );
        super::push(
            inv,
            Dependency {
                ecosystem: Ecosystem::Pypi,
                name: normalized,
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
    let first_dependency = inv.dependencies.len();
    let value: toml::Value = match toml::from_str(text) {
        Ok(value) => value,
        Err(_) => {
            inv.coverage_gaps
                .push(format!("Malformed Python manifest {name:?}"));
            return;
        }
    };
    if let Some(project) = value.get("project") {
        if project
            .get("dynamic")
            .and_then(toml::Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.as_str() == Some("dependencies"))
            })
        {
            inv.coverage_gaps.push(format!("Dynamic Python dependencies in {name:?} require build execution, which is intentionally disabled"));
        }
        if let Some(deps) = project.get("dependencies") {
            strings(name, deps, locked, inv);
        }
        if let Some(groups) = project
            .get("optional-dependencies")
            .and_then(toml::Value::as_table)
        {
            for deps in groups.values() {
                strings(name, deps, locked, inv);
            }
        }
    }
    if let Some(groups) = value
        .get("dependency-groups")
        .and_then(toml::Value::as_table)
    {
        for deps in groups.values() {
            strings(name, deps, locked, inv);
        }
    }
    if let Some(poetry) = value.get("tool").and_then(|v| v.get("poetry")) {
        if let Some(deps) = poetry.get("dependencies") {
            poetry_deps(name, deps, locked, inv);
        }
        if let Some(deps) = poetry.get("dev-dependencies") {
            poetry_deps(name, deps, locked, inv);
        }
        if let Some(groups) = poetry.get("group").and_then(toml::Value::as_table) {
            for group in groups.values() {
                if let Some(deps) = group.get("dependencies") {
                    poetry_deps(name, deps, locked, inv);
                }
            }
        }
    }
    if let Some(sources) = value
        .get("tool")
        .and_then(|v| v.get("uv"))
        .and_then(|v| v.get("sources"))
        .and_then(toml::Value::as_table)
    {
        for dependency in &mut inv.dependencies[first_dependency..] {
            if sources
                .keys()
                .any(|source| normalize(source) == dependency.name)
            {
                dependency.public_registry = false;
            }
        }
    }
}

fn strings(name: &str, value: &toml::Value, locked: &[Locked], inv: &mut Inventory) {
    if let Some(items) = value.as_array() {
        for item in items {
            if let Some(spec) = item.as_str() {
                requirement(name, spec, locked, inv);
            } else {
                inv.coverage_gaps.push(format!(
                    "Unsupported Python dependency/include record in {name:?}"
                ));
            }
        }
    } else {
        inv.coverage_gaps
            .push(format!("Invalid Python dependency array in {name:?}"));
    }
}

fn poetry_deps(name: &str, value: &toml::Value, locked: &[Locked], inv: &mut Inventory) {
    let Some(deps) = value.as_table() else {
        inv.coverage_gaps
            .push(format!("Invalid Poetry dependencies in {name:?}"));
        return;
    };
    for (pkg, spec) in deps {
        if pkg == "python" {
            continue;
        }
        let public = spec.as_str().is_some()
            || spec.as_table().is_some_and(|t| {
                !["git", "path", "url", "source"]
                    .iter()
                    .any(|key| t.contains_key(*key))
            });
        let range = spec
            .as_str()
            .or_else(|| spec.get("version").and_then(toml::Value::as_str))
            .map(str::to_string);
        if !spec.is_str() && !spec.is_table() {
            inv.coverage_gaps
                .push(format!("Unsupported Poetry dependency {pkg:?} in {name:?}"));
        }
        add(
            inv,
            Ecosystem::Pypi,
            &normalize(pkg),
            range,
            "direct",
            name,
            public,
            locked,
        );
    }
}

pub(super) fn requirements(name: &str, text: &str, locked: &[Locked], inv: &mut Inventory) {
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        if line.starts_with('-') {
            inv.coverage_gaps.push(format!("Requirements option/include in {name:?} is not executed; declare missing libraries explicitly"));
            continue;
        }
        requirement(name, line, locked, inv);
    }
}

fn requirement(name: &str, spec: &str, locked: &[Locked], inv: &mut Inventory) {
    let Ok(pattern) = regex::Regex::new(r"^([A-Za-z0-9][A-Za-z0-9._-]*)(?:\[[^\]]*\])?(.*)$")
    else {
        return;
    };
    let Some(parts) = pattern.captures(spec) else {
        inv.coverage_gaps
            .push(format!("Unrecognized Python requirement in {name:?}"));
        return;
    };
    let pkg = normalize(&parts[1]);
    let range = parts[2].trim();
    add(
        inv,
        Ecosystem::Pypi,
        &pkg,
        (!range.is_empty()).then(|| range.into()),
        "direct",
        name,
        !range.contains('@') && !range.contains("://"),
        locked,
    );
}

fn normalize(name: &str) -> String {
    let mut result = String::new();
    for byte in name.bytes() {
        let ch = if matches!(byte, b'-' | b'_' | b'.') {
            '-'
        } else {
            (byte as char).to_ascii_lowercase()
        };
        if ch != '-' || !result.ends_with('-') {
            result.push(ch);
        }
    }
    result
}

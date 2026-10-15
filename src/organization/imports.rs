//! Bounded textual relative-import resolution; no module execution or filesystem reads.
use super::{OrganizationPolicy, OrganizationReport, finding, under};
mod lexical;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn validate_aliases(aliases: &BTreeMap<String, String>) -> Result<(), String> {
    if aliases.len() > 100 {
        return Err("At most 100 import aliases are supported".into());
    }
    for (prefix, destination) in aliases {
        if prefix.len() > 256
            || !prefix.ends_with('/')
            || prefix.starts_with(['.', '/'])
            || prefix.contains(['\\', ':'])
            || prefix.contains("//")
            || prefix.chars().any(char::is_whitespace)
            || prefix.split('/').any(|part| part == "." || part == "..")
            || destination.len() > 1024
            || !super::valid_path(destination.trim_end_matches('/'), false)
            || destination.ends_with("//")
        {
            return Err(format!(
                "Invalid import alias {prefix:?} -> {destination:?}; require a non-relative namespace ending / and a normalized repository-relative destination"
            ));
        }
    }
    Ok(())
}

fn layer<'a>(path: &str, policy: &'a OrganizationPolicy) -> Option<&'a super::Layer> {
    policy
        .layers
        .iter()
        .filter(|l| under(path, &l.prefix))
        .max_by_key(|l| l.prefix.len())
}

fn resolve(path: &str, specifier: &str, paths: &BTreeSet<&str>) -> Option<String> {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    let base = parts.join("/");
    let mut candidates = vec![base.clone()];
    for extension in [
        "ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "svelte", "rs", "py", "json", "css", "scss",
    ] {
        candidates.push(format!("{base}.{extension}"));
        candidates.push(format!("{base}/index.{extension}"));
    }
    // TypeScript commonly writes .js specifiers for emitted .ts modules.
    if let Some(stem) = base.strip_suffix(".js") {
        candidates.extend([format!("{stem}.ts"), format!("{stem}.tsx")]);
    }
    candidates.extend([format!("{base}/mod.rs"), format!("{base}/__init__.py")]);
    candidates.into_iter().find(|p| paths.contains(p.as_str()))
}

pub(super) fn audit(
    files: &[(String, String)],
    policy: &OrganizationPolicy,
    paths: &BTreeSet<&str>,
    report: &mut OrganizationReport,
) {
    for (path, text) in files {
        let source_layer = layer(path, policy);
        if source_layer.is_none() && !policy.layers.is_empty() {
            report.recommendations.push(finding("undeclared_purpose", path, None, "File is outside every declared layer; its architectural purpose and dependency permissions are unverified"));
        }
        let (specifiers, gaps) = lexical::imports(text);
        for line in gaps {
            report.imports.unparsed_import_lines += 1;
            report.recommendations.push(finding("unparsed_import", path, Some(line), "Dynamic/computed loading, escaped module names, ambiguous slash syntax or template interpolation require manual boundary review; static import checks do not establish compliance for this line"));
        }
        for (line, specifier) in specifiers {
            let specifier = specifier.as_str();
            let alias = policy
                .import_aliases
                .iter()
                .filter_map(|(prefix, destination)| {
                    specifier.strip_prefix(prefix).map(|suffix| {
                        (
                            prefix.len(),
                            format!("{}/{suffix}", destination.trim_end_matches('/')),
                        )
                    })
                })
                .max_by_key(|(length, _)| *length);
            if !specifier.starts_with('.') {
                if let Some(source) = source_layer
                    && let Some(forbidden) = source.forbidden_imports.iter().find(|package| {
                        specifier == package.as_str()
                            || specifier
                                .strip_prefix(package.as_str())
                                .is_some_and(|suffix| suffix.starts_with('/'))
                    })
                {
                    report.violations.push(finding("forbidden_import", path, Some(line), format!("Layer {} forbids package/subpath {forbidden:?}, imported as {specifier:?}", source.name)));
                }
                if alias.is_none() {
                    report.imports.external_imports += 1;
                    continue;
                }
            }
            let resolved = if let Some((_, mapped)) = alias {
                report.imports.alias_imports += 1;
                resolve("", &mapped, paths)
            } else {
                report.imports.relative_imports += 1;
                resolve(path, specifier, paths)
            };
            let Some(target) = resolved else {
                report.imports.unresolved_imports += 1;
                report.recommendations.push(finding("unresolved_relative_import", path, Some(line), format!("Cannot resolve relative/configured-alias import {specifier:?} within supplied inventory; boundary remains unverified")));
                continue;
            };
            report.imports.resolved_imports += 1;
            match (source_layer, layer(&target, policy)) {
                (Some(source), Some(destination)) => {
                    if source.name != destination.name
                        && !source.allow_imports_from.contains(&destination.name)
                    {
                        report.violations.push(finding("layer_dependency", path, Some(line), format!("{} imports {} via {specifier:?} ({target}); direction is not allowed", source.name, destination.name)));
                    }
                }
                _ => report.imports.unclassified_edges += 1,
            }
        }
    }
}

//! Explicit migration evidence and a bounded caller-selected comment corpus.
use super::{DebtItem, Ecosystem, ranking::estimate};
use serde::Deserialize;
use std::path::{Component, Path};

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Migration {
    ecosystem: Ecosystem,
    name: String,
    from: String,
    to: String,
    effort: u8,
    summary: String,
    source_url: String,
    #[serde(default)]
    required_env_vars: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Maintenance {
    id: String,
    summary: String,
    urgency: String,
    effort: Option<u8>,
    source_url: String,
    due_date: Option<String>,
}

fn bounded(path: &Path, limit: usize) -> Result<String, String> {
    use std::io::Read;
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit as u64 {
        return Err("Maintenance input must be a bounded regular file".into());
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > limit {
        return Err("Maintenance input grew beyond byte budget".into());
    }
    Ok(text)
}

fn evidence(summary: &str, url: &str, effort: Option<u8>) -> Result<(), String> {
    let url = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    if summary.trim().len() < 12
        || summary.len() > 4096
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || effort.is_some_and(|rating| !(1..=5).contains(&rating))
    {
        return Err("Maintenance evidence needs a concrete summary, HTTPS source and effort 1..5 (or omitted)".into());
    }
    Ok(())
}

pub(super) fn apply(
    base: &Path,
    sources: &[String],
    items: &mut Vec<DebtItem>,
    gaps: &mut Vec<String>,
) -> Result<(), String> {
    let root = base.canonicalize().map_err(|e| e.to_string())?;
    let policy = root.join(".kritic/debt.toml");
    if policy.exists() {
        if !policy
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(&root)
        {
            return Err("Debt policy resolves outside the project".into());
        }
        let value: toml::Value =
            toml::from_str(&bounded(&policy, 2_097_152)?).map_err(|e| e.to_string())?;
        if value.get("version").and_then(toml::Value::as_integer) != Some(1) {
            return Err("Debt policy version must be 1".into());
        }
        let migrations: Vec<Migration> = value
            .get("migrations")
            .cloned()
            .map(|v| v.try_into())
            .transpose()
            .map_err(|e: toml::de::Error| e.to_string())?
            .unwrap_or_default();
        let maintenance: Vec<Maintenance> = value
            .get("maintenance")
            .cloned()
            .map(|v| v.try_into())
            .transpose()
            .map_err(|e: toml::de::Error| e.to_string())?
            .unwrap_or_default();
        if migrations.len() > 100 || maintenance.len() > 100 {
            return Err("Debt policy exceeds 100 migration or maintenance notes".into());
        }
        let mut keys = std::collections::BTreeSet::new();
        for note in migrations {
            evidence(&note.summary, &note.source_url, Some(note.effort))?;
            if note.name.is_empty()
                || note.from.is_empty()
                || note.to.is_empty()
                || !keys.insert((
                    note.ecosystem,
                    note.name.clone(),
                    note.from.clone(),
                    note.to.clone(),
                ))
            {
                return Err("Migration notes need a unique package/from/to identity".into());
            }
            if note.required_env_vars.len() > 64
                || note.required_env_vars.iter().any(|name| {
                    name.is_empty()
                        || name.len() > 128
                        || !name
                            .bytes()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
                        || name.as_bytes()[0].is_ascii_digit()
                })
            {
                return Err(
                    "Migration environment requirements must be variable names, never values"
                        .into(),
                );
            }
            let mut matched = false;
            for item in items.iter_mut().filter(|item| {
                item.ecosystem == Some(note.ecosystem)
                    && item.package.as_deref() == Some(note.name.as_str())
                    && item.current.as_deref() == Some(note.from.as_str())
                    && item.latest.as_deref() == Some(note.to.as_str())
            }) {
                matched = true;
                item.effort = estimate(
                    Some(note.effort),
                    "Declared version-bound migration plan; independently verify",
                );
                item.effort.evidence.push(format!(
                    "Declared note: {} ({})",
                    note.summary, note.source_url
                ));
                if !note.required_env_vars.is_empty() {
                    item.effort.evidence.push(format!(
                        "Declared required environment-variable names: {} (values not collected)",
                        note.required_env_vars.join(", ")
                    ));
                }
            }
            if !matched {
                gaps.push(format!("Migration note for {} {} -> {} did not match an observed dependency target; it was not applied", note.name, note.from, note.to));
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for note in maintenance {
            evidence(&note.summary, &note.source_url, note.effort)?;
            if note.id.is_empty()
                || !ids.insert(note.id.clone())
                || !matches!(
                    note.urgency.as_str(),
                    "low" | "medium" | "high" | "critical"
                )
            {
                return Err(
                    "Maintenance notes need unique ids and low/medium/high/critical urgency".into(),
                );
            }
            let mut urgency = note.urgency.clone();
            let mut reason = "Project-declared maintenance evidence; source contents have not been fetched or authenticated".to_string();
            if let Some(date) = note.due_date {
                let due = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                    .map_err(|e| e.to_string())?;
                reason.push_str(&format!("; declared due date {date}"));
                if due < chrono::Local::now().date_naive()
                    && super::urgency_rank(&urgency) < super::urgency_rank("high")
                {
                    urgency = "high".into();
                    reason.push_str(" has passed");
                }
            }
            let mut effort = estimate(note.effort, "Project-declared effort; independently verify");
            effort.evidence.push(note.source_url);
            items.push(DebtItem {
                id: super::identity(&format!("maintenance:{}", note.id)),
                category: "declared_maintenance".into(),
                summary: note.summary,
                urgency,
                urgency_reason: reason,
                ecosystem: None,
                package: None,
                current: None,
                latest: None,
                version_status: "not_applicable".into(),
                source: ".kritic/debt.toml".into(),
                source_line: None,
                effort,
                lookup: None,
            });
        }
    }
    if sources.len() > 50 {
        return Err("At most 50 explicit maintenance source paths are allowed".into());
    }
    let mut total = 0;
    let mut count = 0;
    for relative in sources {
        let path = Path::new(relative);
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(
                "Maintenance sources must be explicit relative paths without traversal".into(),
            );
        }
        let local = root.join(path);
        let result = (|| {
            if !local
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(&root)
            {
                return Err("Source resolves outside project".into());
            }
            bounded(&local, 2_097_152)
        })();
        let text = match result {
            Ok(text) => text,
            Err(error) => {
                gaps.push(format!("Maintenance source {relative}: {error}"));
                continue;
            }
        };
        total += text.len();
        if total > 16_777_216 {
            return Err("Maintenance sources exceed 16 MiB".into());
        }
        for (line, content) in text.lines().enumerate() {
            let content = content.trim_start();
            let comment = ["//", "#", "/*", "*", "<!--"]
                .into_iter()
                .find_map(|prefix| content.strip_prefix(prefix));
            let Some(comment) = comment.map(str::trim_start) else {
                continue;
            };
            let marker = ["TODO", "FIXME", "HACK", "DEPRECATED"]
                .into_iter()
                .find(|tag| {
                    comment.starts_with(tag)
                        && comment
                            .as_bytes()
                            .get(tag.len())
                            .is_none_or(|c| !c.is_ascii_alphanumeric() && *c != b'_')
                });
            let Some(marker) = marker else {
                continue;
            };
            count += 1;
            if count > 1000 {
                return Err("Maintenance comment corpus exceeds 1000 findings".into());
            }
            items.push(DebtItem {
                id: super::identity(&format!("comment:{relative}:{line}:{comment}")),
                category: "maintenance_comment".into(),
                summary: comment.chars().take(240).collect(),
                urgency: if marker == "FIXME" { "medium" } else { "low" }.into(),
                urgency_reason:
                    "Comment marker is a review lead, not proof of a defect or its urgency".into(),
                ecosystem: None,
                package: None,
                current: None,
                latest: None,
                version_status: "not_applicable".into(),
                source: relative.clone(),
                source_line: Some(line + 1),
                effort: estimate(
                    None,
                    "Read the code and acceptance criteria before estimating",
                ),
                lookup: None,
            });
        }
    }
    Ok(())
}

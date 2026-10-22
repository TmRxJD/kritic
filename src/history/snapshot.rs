use super::{InputHash, Snapshot};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};

pub(super) fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort();
            let mut sorted = serde_json::Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonical(&object[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}

pub(super) fn bytes<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&canonical(
        &serde_json::to_value(value).map_err(|e| e.to_string())?,
    ))
    .map_err(|e| e.to_string())
}

pub(super) fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let info =
        std::fs::symlink_metadata(path).map_err(|e| format!("Read {}: {e}", path.display()))?;
    if !info.is_file() || info.file_type().is_symlink() {
        return Err(format!("Expected regular file: {}", path.display()));
    }
    if info.len() > limit as u64 {
        return Err(format!(
            "File exceeds {limit} byte limit: {}",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!(
            "File exceeds {limit} byte limit: {}",
            path.display()
        ));
    }
    Ok(bytes)
}

pub(super) fn repository(repo: &Path) -> Result<(PathBuf, String, Option<String>), String> {
    let root = crate::proc::hidden_command("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| e.to_string())?;
    if !root.status.success() {
        return Err(format!(
            "Resolve Git repository: {}",
            String::from_utf8_lossy(&root.stderr).trim()
        ));
    }
    let path = PathBuf::from(
        std::str::from_utf8(&root.stdout)
            .map_err(|e| e.to_string())?
            .trim_end_matches(['\r', '\n']),
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    let identity = path
        .to_str()
        .ok_or("Repository path must be UTF-8")?
        .to_string();
    #[cfg(windows)]
    let identity = identity.to_lowercase();
    let head = crate::proc::hidden_command("git")
        .arg("-C")
        .arg(&path)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|e| e.to_string())?;
    let revision = if head.status.success() {
        Some(
            String::from_utf8(head.stdout)
                .map_err(|e| e.to_string())?
                .trim()
                .into(),
        )
    } else {
        None
    };
    Ok((path, hash(identity.as_bytes()), revision))
}

pub(super) fn capture(
    repo: &Path,
    report: &Path,
    inputs: &[PathBuf],
    context: &str,
) -> Result<Snapshot, String> {
    let context = context.trim();
    if context.len() < 12 || context.len() > 65_536 {
        return Err(
            "Declare concrete tool/model/command/config context between 12 and 65536 bytes".into(),
        );
    }
    if inputs.is_empty() || inputs.len() > 512 {
        return Err("History needs 1..512 explicit input files".into());
    }
    let (root, repository_sha256, revision) = repository(repo)?;
    let report_bytes = read_bounded(report, 8_388_608)?;
    let report: Value =
        serde_json::from_slice(&report_bytes).map_err(|e| format!("Invalid report JSON: {e}"))?;
    let tool = report
        .as_object()
        .and_then(|object| object.get("tool"))
        .and_then(Value::as_str)
        .ok_or("Report must be an object with a Kritic tool name")?
        .to_string();
    if !known_tool(&tool) {
        return Err("Report tool is not a supported Kritic tool identifier".into());
    }
    let mut hashes = std::collections::BTreeMap::new();
    let mut total = 0usize;
    for relative in inputs {
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
            || relative.as_os_str().is_empty()
        {
            return Err(
                "History inputs must be explicit repository-relative files without traversal"
                    .into(),
            );
        }
        let local = root.join(relative);
        let resolved = local.canonicalize().map_err(|e| e.to_string())?;
        if !resolved.starts_with(&root) {
            return Err("History input resolves outside the repository".into());
        }
        let content = read_bounded(&local, 2_097_152)?;
        total += content.len();
        if total > 33_554_432 {
            return Err("History inputs exceed 32 MiB total".into());
        }
        let name = relative
            .components()
            .map(|part| part.as_os_str().to_str().ok_or("Input path must be UTF-8"))
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        if hashes.insert(name, hash(&content)).is_some() {
            return Err("Duplicate explicit history input".into());
        }
    }
    Ok(Snapshot {
        version: 1,
        repository_sha256,
        revision,
        tool,
        context_sha256: hash(context.as_bytes()),
        inputs: hashes
            .into_iter()
            .map(|(path, sha256)| InputHash { path, sha256 })
            .collect(),
        report_sha256: hash(&bytes(&report)?),
        report: canonical(&report),
    })
}

pub(super) fn known_tool(tool: &str) -> bool {
    matches!(
        tool,
        "critic_score"
            | "artifact_validate"
            | "repository_critic"
            | "security_critic"
            | "code_quality_critic"
            | "technical_debt_critic"
            | "framing_critic"
            | "contract_critic"
            | "visual_critic"
            | "ux_critic"
            | "ui_test"
            | "desktop_test"
            | "resource_critic"
            | "hygiene_critic"
            | "map_eval"
            | "unity_tests"
            | "unity_critic"
    )
}

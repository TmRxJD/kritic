use super::*;
use serde::Deserialize;
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
pub(super) fn relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.contains(['\\', ':'])
        && !path.chars().any(char::is_control)
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
pub(super) fn read(base: &Path, path: &str, limit: usize) -> Result<Vec<u8>, String> {
    if !relative(path) {
        return Err(
            "Expected a normalized repository-relative path without traversal, drive or ADS syntax"
                .into(),
        );
    }
    let mut target = PathBuf::from(base);
    for component in Path::new(path).components() {
        target.push(component);
        if std::fs::symlink_metadata(&target)
            .map_err(|error| format!("Read {path}: {error}"))?
            .file_type()
            .is_symlink()
        {
            return Err(format!("Symlink evidence paths are not supported: {path}"));
        }
    }
    let resolved = target.canonicalize().map_err(|error| error.to_string())?;
    if !resolved.starts_with(base) {
        return Err("Evidence path resolves outside base_dir".into());
    }
    let metadata = std::fs::metadata(&resolved).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(format!(
            "Expected regular file at most {limit} bytes: {path}"
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&resolved)
        .map_err(|error| error.to_string())?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err(format!("Evidence grew beyond {limit} bytes: {path}"));
    }
    Ok(bytes)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    context: Context,
    data: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    revision: String,
    run_id: String,
    input_digest: String,
}

pub(super) struct Checked {
    pub result: EvidenceResult,
    pub document: Option<Value>,
}
pub(super) fn check(
    base: &Path,
    evidence: &Evidence,
    case: &Case,
    remaining: &mut usize,
) -> Checked {
    let result = EvidenceResult {
        id: evidence.id.clone(),
        path: evidence.path.clone(),
        kind: evidence.kind,
        verified: false,
        state: "missing".into(),
        detail: String::new(),
    };
    let mut checked = Checked {
        result,
        document: None,
    };
    let bytes = match read(base, &evidence.path, (*remaining).min(1024 * 1024)) {
        Ok(bytes) => bytes,
        Err(error) => {
            checked.result.detail = error;
            return checked;
        }
    };
    *remaining = remaining.saturating_sub(bytes.len());
    if hash(&bytes) != evidence.sha256 {
        checked.result.state = "stale".into();
        checked.result.detail = "Pinned SHA-256 does not match current file bytes".into();
        return checked;
    }
    if evidence.kind == EvidenceKind::Schema {
        let schema: Value = match serde_json::from_slice(&bytes) {
            Ok(schema) => schema,
            Err(error) => {
                checked.result.state = "unverified".into();
                checked.result.detail = format!("Schema evidence is not JSON: {error}");
                return checked;
            }
        };
        if !schema.as_object().is_some_and(|object| {
            [
                "type",
                "$ref",
                "$defs",
                "properties",
                "allOf",
                "anyOf",
                "oneOf",
                "enum",
                "const",
                "items",
            ]
            .iter()
            .any(|key| object.contains_key(*key))
        }) {
            checked.result.state = "unverified".into();
            checked.result.detail="Schema evidence needs a JSON object with declared schema structure; schema semantics are not compiled/validated".into();
            return checked;
        }
        checked.document = Some(schema);
    }
    if evidence.kind == EvidenceKind::Observation {
        let envelope: Envelope = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                checked.result.state = "unverified".into();
                checked.result.detail = format!("Invalid observation envelope: {error}");
                return checked;
            }
        };
        if envelope.context.revision != case.revision
            || envelope.context.run_id != case.run_id
            || envelope.context.input_digest != case.input_digest
        {
            checked.result.state = "stale".into();
            checked.result.detail =
                "Observation context differs from supplied case revision/run_id/input_digest"
                    .into();
            return checked;
        }
        let document = serde_json::json!({"data":envelope.data});
        if evidence.assertions.is_empty() {
            checked.result.state = "unverified".into();
            checked.result.detail = "Observation has no content assertions".into();
            return checked;
        }
        for assertion in &evidence.assertions {
            if document.pointer(&assertion.pointer) != Some(&assertion.expected) {
                checked.result.state = "contradicted".into();
                checked.result.detail = format!(
                    "Observation assertion disagrees with actual content at {}",
                    assertion.pointer
                );
                return checked;
            }
        }
        checked.document = Some(document);
    }
    checked.result.verified = true;
    checked.result.state = "verified".into();
    checked.result.detail = if evidence.kind == EvidenceKind::Observation {
        "Pinned bytes, asserted data and supplied context agree; execution, causality and current HEAD are not attested".into()
    } else {
        "File bytes match the supplied hash; runtime ownership/schema enforcement is not established".into()
    };
    checked
}

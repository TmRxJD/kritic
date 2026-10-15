//! Bounded immutable-identity cache envelopes; age is metadata, not an accuracy guarantee.
use super::Metadata;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Component, Path},
};

const ENTRY_LIMIT: usize = 256 * 1024;
const STORE_LIMIT: u64 = 64 * 1024 * 1024;
const COUNT_LIMIT: usize = 1024;

struct Lock(std::path::PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    version: u32,
    identity: String,
    observed_at: String,
    payload_sha256: String,
    payload: Metadata,
}

fn hash(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn key(identity: &str) -> String {
    format!("{}.json", hash(identity.as_bytes()))
}

fn directory(root: &Path, create: bool) -> Result<(), String> {
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("Registry cache must be an absolute normalized path".into());
    }
    if create {
        std::fs::create_dir_all(root).map_err(|error| format!("Create registry cache: {error}"))?;
    }
    if root.exists() {
        let metadata = std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("Registry cache root must be a regular directory, not a symlink".into());
        }
    }
    Ok(())
}

pub(super) fn read(root: &Path, identity: &str) -> Result<Option<(Metadata, String, i64)>, String> {
    directory(root, false)?;
    let path = root.join(key(identity));
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Read registry cache: {error}")),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > ENTRY_LIMIT as u64
    {
        return Err("Registry cache entry is nonregular, symlinked or oversized".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(ENTRY_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > ENTRY_LIMIT {
        return Err("Registry cache entry exceeds budget".into());
    }
    let entry: Entry = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid registry cache: {error}"))?;
    if entry.version != 1
        || entry.identity != identity
        || entry.payload_sha256
            != hash(&serde_json::to_vec(&entry.payload).map_err(|error| error.to_string())?)
    {
        return Err("Registry cache identity/integrity mismatch".into());
    }
    let observed = chrono::DateTime::parse_from_rfc3339(&entry.observed_at)
        .map_err(|error| format!("Invalid cache timestamp: {error}"))?;
    let age = chrono::Utc::now()
        .signed_duration_since(observed)
        .num_seconds();
    if age < 0 {
        return Err("Registry cache timestamp is in the future".into());
    }
    Ok(Some((entry.payload, entry.observed_at, age)))
}

pub(super) fn write(
    root: &Path,
    identity: &str,
    payload: &Metadata,
    observed_at: &str,
) -> Result<(), String> {
    directory(root, true)?;
    let lock = root.join(".write-lock");
    std::fs::create_dir(&lock)
        .map_err(|error| format!("Registry cache writer lock unavailable: {error}"))?;
    let _guard = Lock(lock);
    let mut count = 0;
    let mut total = 0u64;
    for item in std::fs::read_dir(root).map_err(|error| error.to_string())? {
        let item = item.map_err(|error| error.to_string())?;
        if item.file_name() == ".write-lock" {
            continue;
        }
        let metadata = item.file_type().map_err(|error| error.to_string())?;
        if metadata.is_symlink() || !metadata.is_file() {
            return Err("Unexpected nonregular registry cache entry".into());
        }
        count += 1;
        total = total.saturating_add(item.metadata().map_err(|error| error.to_string())?.len());
        if count > COUNT_LIMIT || total > STORE_LIMIT {
            return Err("Registry cache exceeds budget; archive explicitly".into());
        }
    }
    let target = root.join(key(identity));
    let previous = if target.exists() {
        std::fs::metadata(&target)
            .map_err(|error| error.to_string())?
            .len()
    } else {
        0
    };
    let entry = Entry {
        version: 1,
        identity: identity.into(),
        observed_at: observed_at.into(),
        payload_sha256: hash(&serde_json::to_vec(payload).map_err(|error| error.to_string())?),
        payload: payload.clone(),
    };
    let bytes = serde_json::to_vec(&entry).map_err(|error| error.to_string())?;
    if bytes.len() > ENTRY_LIMIT
        || total
            .saturating_sub(previous)
            .saturating_add(bytes.len() as u64)
            > STORE_LIMIT
        || (previous == 0 && count >= COUNT_LIMIT)
    {
        return Err("Registry cache entry/count/byte budget exceeded; archive explicitly".into());
    }
    let mut file = tempfile::NamedTempFile::new_in(root).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    file.persist(target)
        .map_err(|error| format!("Atomic registry cache write: {error}"))?;
    Ok(())
}

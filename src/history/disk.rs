use super::{Record, RecordSummary, Snapshot, Store, snapshot};
use std::{io::Write, path::PathBuf};

const MAX_ENTRY: usize = 12_582_912;
const MAX_STORE: u64 = 67_108_864;

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

impl Store {
    fn directory(&self, repository: &str) -> Result<PathBuf, String> {
        if !valid_id(repository) {
            return Err("Invalid repository identity".into());
        }
        Ok(self.root.join(repository))
    }

    fn entries(&self, repository: &str) -> Result<Vec<(String, u64)>, String> {
        let directory = self.directory(repository)?;
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        let mut total = 0u64;
        for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("History entry filename is not UTF-8")?;
            if !name.ends_with(".json") {
                continue;
            }
            let id = name
                .strip_suffix(".json")
                .ok_or("Invalid history filename")?;
            if !valid_id(id) {
                return Err("Invalid history entry filename".into());
            }
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("History entries must be regular files".into());
            }
            let size = entry.metadata().map_err(|e| e.to_string())?.len();
            total = total.checked_add(size).ok_or("History size overflow")?;
            if records.len() >= 512 || total > MAX_STORE {
                return Err(
                    "History exceeds 512 entries or 64 MiB; archive explicitly before continuing"
                        .into(),
                );
            }
            records.push((id.to_string(), size));
        }
        records.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(records)
    }

    pub(super) fn summaries(&self, repository: &str) -> Result<Vec<RecordSummary>, String> {
        self.entries(repository)?
            .into_iter()
            .map(|(id, _)| Ok(self.read(repository, &id)?.summary()))
            .collect()
    }

    pub(super) fn read(&self, repository: &str, id: &str) -> Result<Record, String> {
        if !valid_id(id) {
            return Err("History id must be 64 lowercase hexadecimal characters".into());
        }
        let raw = snapshot::read_bounded(
            &self.directory(repository)?.join(format!("{id}.json")),
            MAX_ENTRY,
        )?;
        let record: Record =
            serde_json::from_slice(&raw).map_err(|e| format!("Invalid history record: {e}"))?;
        let expected = snapshot::hash(&snapshot::bytes(&record.snapshot)?);
        let integrity = snapshot::hash(&snapshot::bytes(&(
            &record.id,
            &record.created_at,
            &record.snapshot,
        ))?);
        if record.id != id
            || expected != id
            || record.snapshot.repository_sha256 != repository
            || record.integrity_sha256 != integrity
            || record.snapshot.version != 1
            || snapshot::hash(&snapshot::bytes(&record.snapshot.report)?)
                != record.snapshot.report_sha256
            || !snapshot::known_tool(&record.snapshot.tool)
        {
            return Err(
                "History integrity check failed; record is historical and cannot be trusted".into(),
            );
        }
        chrono::DateTime::parse_from_rfc3339(&record.created_at)
            .map_err(|e| format!("Invalid history timestamp: {e}"))?;
        Ok(record)
    }

    pub(super) fn save(&self, snapshot: Snapshot) -> Result<Record, String> {
        let directory = self.directory(&snapshot.repository_sha256)?;
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let lock = directory.join(".write-lock");
        std::fs::create_dir(&lock).map_err(|e| {
            format!("History writer lock unavailable (remove a stale lock explicitly): {e}")
        })?;
        let _guard = Lock(lock);
        let id = snapshot::hash(&snapshot::bytes(&snapshot)?);
        let target = directory.join(format!("{id}.json"));
        if target.exists() {
            return self.read(&snapshot.repository_sha256, &id);
        }
        let existing = self.entries(&snapshot.repository_sha256)?;
        if existing.len() >= 512 {
            return Err("History has 512 entries; archive explicitly before continuing".into());
        }
        let created_at = chrono::Utc::now().to_rfc3339();
        let integrity_sha256 = snapshot::hash(&snapshot::bytes(&(&id, &created_at, &snapshot))?);
        let record = Record {
            id,
            created_at,
            snapshot,
            integrity_sha256,
        };
        let bytes = snapshot::bytes(&record)?;
        let current: u64 = existing.iter().map(|(_, size)| size).sum();
        if bytes.len() > MAX_ENTRY || current + bytes.len() as u64 > MAX_STORE {
            return Err(
                "History entry/store byte budget exceeded; archive explicitly before continuing"
                    .into(),
            );
        }
        let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist_noclobber(target)
            .map_err(|e| format!("Write immutable history entry: {e}"))?;
        Ok(record)
    }
}

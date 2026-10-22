//! Explicit fixture projection only. Never rewrites tests, review hashes or contracts.
use std::io::Write;
use std::path::Path;

use super::{SyncReport, audit, files, manifest};

pub fn sync(base: &Path, path: Option<&str>, write: bool) -> Result<SyncReport, String> {
    let base = base.canonicalize().map_err(|e| e.to_string())?;
    let path = path.unwrap_or(super::DEFAULT_MANIFEST);
    let (m, _) = manifest::load(&base, path)?;
    let current = super::check(&base, Some(path))?;
    let mut report = SyncReport { written: vec![], unchanged: vec![], would_update: vec![], review_required: false, note: "Only declared JSON fixtures are projected. No tests, review hashes or authorities changed. Re-run staleness check and behavioral validators; generation is not semantic approval.".into() };
    report.review_required = current.findings.iter().any(|f| {
        !matches!(
            f.code.as_str(),
            "generated_fixture_drift" | "generated_fixture_unavailable"
        )
    });
    let mut budget = files::MAX_TOTAL;
    let mut planned = Vec::new();
    // Validate every source and destination before the first write. Atomic per file, not a
    // cross-file transaction: a later I/O failure must be repaired and check rerun.
    for fixture in &m.generated {
        let authority = m
            .authorities
            .iter()
            .find(|a| a.id == fixture.authority)
            .ok_or("Unknown fixture authority")?;
        let source = files::read(&base, &authority.path, &mut budget)?;
        let source_hash = crate::contract::hash(&source);
        report.review_required |= m
            .tests
            .iter()
            .filter(|t| fixture.consumers.contains(&t.path))
            .any(|t| t.reviewed_inputs.get(&fixture.authority) != Some(&source_hash));
        let bytes = audit::fixture_bytes(&source, &fixture.pointer)?;
        let dest = files::resolve(&base, &fixture.path, true)?;
        if dest == files::resolve(&base, path, false)? {
            return Err("Generated fixture cannot overwrite the manifest".into());
        }
        match files::read(&base, &fixture.path, &mut budget) {
            Ok(old) if old == bytes => {
                report.unchanged.push(fixture.path.clone());
                continue;
            }
            Ok(_) => {}
            Err(error) => match std::fs::symlink_metadata(&dest) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(error),
            },
        }
        report.would_update.push(fixture.path.clone());
        planned.push((fixture.path.clone(), dest, bytes));
    }
    if write {
        for (name, dest, bytes) in planned {
            let parent = dest.parent().ok_or("Fixture parent missing")?;
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            // Recheck after parent creation; refuse a changed symlink destination.
            files::resolve(&base, &name, true)?;
            let mut temporary =
                tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
            temporary.write_all(&bytes).map_err(|e| e.to_string())?;
            temporary.as_file().sync_all().map_err(|e| e.to_string())?;
            temporary.persist(dest).map_err(|e| e.to_string())?;
            report.written.push(name);
        }
    }
    Ok(report)
}

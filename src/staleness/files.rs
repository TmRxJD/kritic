//! Bounded repository reads; no external paths, generated trees or symlink traversal.
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub(super) const MAX_FILE: usize = 1024 * 1024;
pub(super) const MAX_TOTAL: usize = 32 * MAX_FILE;

#[cfg(windows)]
fn reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn reparse(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

pub(super) fn relative(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains('\\')
        || path.contains(':')
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(format!(
            "Use a nonempty relative path with forward slashes: {path}"
        ));
    }
    for part in Path::new(path).components() {
        let Component::Normal(name) = part else {
            return Err(format!(
                "Path must contain only normal relative components: {path}"
            ));
        };
        if forbidden(&name.to_string_lossy()) {
            return Err(format!(
                "Excluded generated/private directory in path: {path}"
            ));
        }
    }
    Ok(())
}

pub(super) fn forbidden(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | "node_modules"
            | "dist"
            | "build"
            | "target"
            | "debug"
            | ".turbo"
            | "coverage"
            | "graphify-out"
            | ".venv"
            | "__pycache__"
    )
}

/// Check each existing component, including Windows junctions, before reading or writing.
pub(super) fn resolve(base: &Path, path: &str, missing: bool) -> Result<PathBuf, String> {
    relative(path)?;
    let mut current = base.to_path_buf();
    for component in Path::new(path).components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                if reparse(&meta) {
                    return Err(format!("Symlink/reparse paths are not supported: {path}"));
                }
                let canonical = current.canonicalize().map_err(|e| e.to_string())?;
                if !canonical.starts_with(base) {
                    return Err(format!("Path escapes repository: {path}"));
                }
            }
            Err(e) if missing && e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{path}: {e}")),
        }
    }
    Ok(current)
}

pub(super) fn read(base: &Path, path: &str, budget: &mut usize) -> Result<Vec<u8>, String> {
    let full = resolve(base, path, false)?;
    let meta = full.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_FILE as u64 || meta.len() > *budget as u64 {
        return Err(format!("Not a regular file or exceeds 1 MiB: {path}"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(full)
        .map_err(|e| e.to_string())?
        .take((MAX_FILE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE || bytes.len() > *budget {
        return Err("Staleness read budget exceeded (1 MiB/file, 32 MiB total)".into());
    }
    *budget -= bytes.len();
    Ok(bytes)
}

pub(super) fn discover(
    base: &Path,
    roots: &[String],
    pattern: &regex::Regex,
) -> Result<Vec<String>, String> {
    let mut pending = Vec::new();
    for root in roots {
        let path = resolve(base, root, false)?;
        if !path.is_dir() {
            return Err(format!("Test root is not a directory: {root}"));
        }
        pending.push(path);
    }
    let mut visited = std::collections::BTreeSet::new();
    let mut found = std::collections::BTreeSet::new();
    let mut entries = 0;
    while let Some(dir) = pending.pop() {
        if !visited.insert(dir.clone()) {
            continue;
        }
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            entries += 1;
            if entries > 5000 {
                return Err("Test discovery exceeds 5000 entries; narrow test_roots".into());
            }
            if forbidden(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let path = entry.path();
            let name = path
                .strip_prefix(base)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            resolve(base, &name, false)?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() && pattern.is_match(&name) {
                found.insert(name);
            }
        }
    }
    Ok(found.into_iter().collect())
}

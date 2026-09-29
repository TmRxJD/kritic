//! Path resolution and filtering for whole-task artifact sets.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub const MAX_FILE_BYTES: u64 = 400_000;
pub const MAX_FILES: usize = 80;

const SKIP_DIRS: &[&str] = &[
    "node_modules", "dist", "build", ".turbo", "coverage", ".git", "graphify-out", ".venv", "__pycache__",
    "target", ".next", ".svelte-kit",
];
const BINARY_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "pdf", "zip", "gz", "exe", "dll", "so", "dylib", "bin", "gguf",
    "safetensors", "pt", "onnx", "mp3", "mp4", "wav", "ogg", "egg", "woff", "woff2", "ttf", "lock",
];

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

pub fn kind_for_path(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("json") => "json",
        Some("yaml" | "yml") => "yaml",
        Some("toml") => "toml",
        Some("py") => "python",
        Some("md") => "markdown",
        Some("dat" | "beatmap") => "map",
        Some(
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "gleam" | "rs" | "go" | "cs" | "svelte" | "vue" | "sh"
            | "ps1" | "sql" | "css" | "html" | "c" | "cpp" | "h" | "hpp" | "java" | "kt" | "swift" | "lua",
        ) => "code",
        _ => "text",
    }
}

/// Resolve paths (relative ones against `base`) and drop anything the critic shouldn't read.
pub fn resolve_paths(paths: &[String], base: Option<&str>) -> (Vec<PathBuf>, Vec<Skipped>) {
    let base = base.map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let mut usable = Vec::new();
    let mut skipped = Vec::new();
    let mut seen = HashSet::new();
    for raw in paths {
        let p = PathBuf::from(raw);
        let p = if p.is_absolute() { p } else { base.join(p) };
        let p = dunce_canonical(&p);
        if !seen.insert(p.clone()) {
            continue;
        }
        let reason = if !p.exists() {
            Some("not found")
        } else if p.is_dir() {
            Some("is a directory (pass files)")
        } else if p.components().any(|c| SKIP_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref())) {
            Some("in ignored directory")
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| BINARY_EXT.contains(&e.to_ascii_lowercase().as_str()))
        {
            Some("binary/lockfile")
        } else if p.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false) {
            Some("larger than 400 KB")
        } else if usable.len() >= MAX_FILES {
            Some("over 80 file limit")
        } else {
            None
        };
        match reason {
            Some(r) => skipped.push(Skipped { path: p.display().to_string(), reason: r.into() }),
            None => usable.push(p),
        }
    }
    (usable, skipped)
}

/// Canonicalize without the `\\?\` prefix Windows adds; fall back to the input if it doesn't exist.
pub fn dunce_canonical(p: &Path) -> PathBuf {
    match std::fs::canonicalize(p) {
        Ok(c) => {
            let s = c.display().to_string();
            PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
        }
        Err(_) => p.to_path_buf(),
    }
}

/// Read as UTF-8, replacing invalid bytes (flagged later by the utf8 check).
pub fn read_lossy(p: &Path) -> std::io::Result<String> {
    Ok(String::from_utf8_lossy(&std::fs::read(p)?).into_owned())
}

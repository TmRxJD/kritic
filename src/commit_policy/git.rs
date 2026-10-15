//! Git plumbing uses NUL-separated paths; invalid UTF-8 is rejected, never lossy-mapped.
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) fn output(base: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let result = Command::new("git")
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(base)
        .args(args)
        .output()
        .map_err(|e| format!("Run git: {e}"))?;
    if !result.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(result.stdout)
}

fn text(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes)
        .map_err(|_| "Git path or text is not UTF-8; cannot audit safely".into())
}

fn paths(bytes: Vec<u8>) -> Result<Vec<String>, String> {
    bytes
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| text(b.to_vec()))
        .collect()
}

pub(super) fn staged_paths(base: &Path) -> Result<Vec<String>, String> {
    if !output(base, &["ls-files", "--unmerged", "-z"])?.is_empty() {
        return Err("Unmerged index entries must be resolved before policy review".into());
    }
    paths(output(
        base,
        &[
            "diff",
            "--cached",
            "--name-only",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "-z",
            "--",
        ],
    )?)
}

pub(super) fn tree(base: &Path) -> Result<String, String> {
    Ok(text(output(base, &["write-tree"])?)?.trim().into())
}

pub(super) fn plan_path(base: &Path) -> Result<PathBuf, String> {
    let raw = text(output(
        base,
        &["rev-parse", "--git-path", "kritic-scope-plan.json"],
    )?)?;
    let path = PathBuf::from(raw.trim_end_matches(['\r', '\n']));
    Ok(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

pub(super) fn line_counts(base: &Path) -> Result<(usize, usize, Vec<String>), String> {
    let raw = output(
        base,
        &[
            "diff",
            "--cached",
            "--numstat",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "-z",
            "--",
        ],
    )?;
    let mut added = 0usize;
    let mut deleted = 0usize;
    let mut binary = Vec::new();
    for record in raw.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let mut fields = record.splitn(3, |b| *b == b'\t');
        let a = fields.next().ok_or("Missing added count")?;
        let d = fields.next().ok_or("Missing deleted count")?;
        let path = text(fields.next().ok_or("Missing numstat path")?.to_vec())?;
        if a == b"-" || d == b"-" {
            binary.push(path);
            continue;
        }
        let count = |v: &[u8]| -> Result<usize, String> {
            std::str::from_utf8(v)
                .map_err(|e| e.to_string())?
                .parse()
                .map_err(|e| format!("Invalid Git line count: {e}"))
        };
        added = added.checked_add(count(a)?).ok_or("Added count overflow")?;
        deleted = deleted
            .checked_add(count(d)?)
            .ok_or("Deleted count overflow")?;
    }
    Ok((added, deleted, binary))
}

pub(super) fn patch_bytes(base: &Path) -> Result<u64, String> {
    // Count a stream rather than retaining or spooling a potentially huge binary patch.
    let mut child = Command::new("git")
        .arg("-C")
        .arg(base)
        .args([
            "diff",
            "--cached",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().ok_or("Missing Git patch stdout")?;
    let bytes = std::io::copy(&mut stdout, &mut std::io::sink()).map_err(|e| e.to_string())?;
    let result = child.wait_with_output().map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(format!(
            "Read staged patch: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(bytes)
}

pub(super) fn binary_bytes(base: &Path, paths: &[String]) -> Result<u64, String> {
    let mut total = 0u64;
    let has_head = output(base, &["rev-parse", "--verify", "HEAD"]).is_ok();
    for path in paths {
        let indexed = !output(base, &["ls-files", "--stage", "-z", "--", path])?.is_empty();
        let spec = if indexed {
            format!(":{path}")
        } else {
            format!("HEAD:{path}")
        };
        let mut bytes = text(output(base, &["cat-file", "-s", &spec])?)?
            .trim()
            .parse::<u64>()
            .map_err(|e| e.to_string())?;
        if indexed
            && has_head
            && !output(base, &["ls-tree", "-z", "--name-only", "HEAD", "--", path])?.is_empty()
        {
            let old = text(output(base, &["cat-file", "-s", &format!("HEAD:{path}")])?)?
                .trim()
                .parse::<u64>()
                .map_err(|e| e.to_string())?;
            bytes = bytes.max(old);
        }
        total = total
            .checked_add(bytes)
            .ok_or("Binary byte count overflow")?;
    }
    Ok(total)
}

fn source(path: &str) -> bool {
    let excluded = [
        "node_modules",
        "dist",
        "build",
        ".turbo",
        "coverage",
        "target",
        "vendor",
        ".git",
    ];
    if path.split('/').any(|part| excluded.contains(&part)) || path.contains("graphify-out/cache/")
    {
        return false;
    }
    matches!(
        Path::new(path).extension().and_then(|v| v.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "vue"
                | "svelte"
                | "py"
                | "go"
                | "java"
                | "kt"
                | "kts"
                | "c"
                | "h"
                | "cpp"
                | "hpp"
                | "cs"
                | "rb"
                | "php"
                | "swift"
                | "sh"
        )
    )
}

pub(super) fn source_files(
    base: &Path,
    staged_only: bool,
    prefixes: &[String],
) -> Result<super::SourceInventory, String> {
    // Resolve both sides so a legitimate project-root junction is supported while
    // a tracked path through an internal junction cannot read outside that root.
    let resolved_root = base
        .canonicalize()
        .map_err(|e| format!("Resolve repository root: {e}"))?;
    if !output(base, &["ls-files", "--unmerged", "-z"])?.is_empty() {
        return Err("Unmerged index entries must be resolved before source review".into());
    }
    let names = if staged_only {
        staged_paths(base)?
    } else {
        paths(output(base, &["ls-files", "-z"])?)?
    };
    let mut files = Vec::new();
    let mut excluded_paths = Vec::new();
    let mut coverage_errors = Vec::new();
    let mut source_warnings = Vec::new();
    let mut total = 0usize;
    for path in names {
        if !source(&path)
            || prefixes.iter().any(|prefix| {
                path == *prefix
                    || path
                        .strip_prefix(prefix.as_str())
                        .is_some_and(|tail| tail.starts_with('/'))
            })
        {
            excluded_paths.push(path);
            continue;
        }
        match source_text(base, &resolved_root, &path, staged_only) {
            Ok(Some(contents)) => {
                if contents.contains('\0') {
                    source_warnings.push(format!("{path:?} contains NUL; UTF-8 text audited, use the project parser to validate syntax"));
                }
                if contents.len() > 134_217_728 - total {
                    let error = format!(
                        "Source coverage incomplete: {path:?} exceeds remaining 128 MiB audit budget"
                    );
                    if staged_only {
                        return Err(error);
                    }
                    coverage_errors.push(error);
                } else {
                    total += contents.len();
                    files.push((path, contents));
                }
            }
            Ok(None) => (),
            Err(error) if staged_only => return Err(error),
            Err(error) => coverage_errors.push(error),
        }
    }
    Ok(super::SourceInventory {
        files,
        excluded_paths,
        coverage_errors,
        source_warnings,
    })
}

/// A failed source read is a named coverage gap, never an implicit exclusion.
fn source_text(
    base: &Path,
    resolved_root: &Path,
    path: &str,
    staged_only: bool,
) -> Result<Option<String>, String> {
    let bytes = if staged_only {
        let spec = format!(":{path}");
        // Deleted paths have no index blob, and therefore contribute no current source.
        let listed = output(base, &["ls-files", "--stage", "-z", "--", path])?;
        if listed.is_empty() {
            return Ok(None);
        }
        if !listed.starts_with(b"100644 ") && !listed.starts_with(b"100755 ") {
            return Err(format!(
                "Source coverage incomplete: {path:?} is not a regular index blob"
            ));
        }
        let size = text(output(base, &["cat-file", "-s", &spec])?)?
            .trim()
            .parse::<usize>()
            .map_err(|e| e.to_string())?;
        if size > 2_097_152 {
            return Err(format!(
                "Source coverage incomplete: {path:?} exceeds 2 MiB"
            ));
        }
        output(base, &["cat-file", "blob", &spec])?
    } else {
        let local = base.join(path);
        let resolved = local
            .canonicalize()
            .map_err(|e| format!("Source coverage incomplete for {path:?}: {e}"))?;
        if !resolved.starts_with(resolved_root) {
            return Err(format!(
                "Source coverage incomplete: {path:?} resolves outside the repository root"
            ));
        }
        let info = std::fs::symlink_metadata(&local)
            .map_err(|e| format!("Source coverage incomplete for {path:?}: {e}"))?;
        if info.file_type().is_symlink() || !info.is_file() {
            return Err(format!(
                "Source coverage incomplete: {path:?} is not a regular file"
            ));
        }
        if info.len() > 2_097_152 {
            return Err(format!(
                "Source coverage incomplete: {path:?} exceeds 2 MiB"
            ));
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(local)
            .map_err(|e| format!("Source coverage incomplete for {path:?}: {e}"))?
            .take(2_097_153)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Source coverage incomplete for {path:?}: {e}"))?;
        if bytes.len() > 2_097_152 {
            return Err(format!(
                "Source coverage incomplete: {path:?} exceeds 2 MiB"
            ));
        }
        bytes
    };
    Ok(Some(text(bytes).map_err(|error| {
        format!("Source coverage incomplete for {path:?}: {error}")
    })?))
}

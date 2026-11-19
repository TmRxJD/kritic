use crate::types::FileRef;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn export_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
}

pub fn resolve(base: &Path, path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty()
        || path.starts_with("\\\\")
        || path.starts_with("//")
        || path.chars().any(char::is_control)
    {
        return Err("Expected an explicit local evidence path".into());
    }
    let given = Path::new(path);
    let full = base
        .join(given)
        .canonicalize()
        .map_err(|e| format!("{path}: {e}"))?;
    if !given.is_absolute() && !full.starts_with(base) {
        return Err(format!("Relative evidence escapes base_dir: {path}"));
    }
    Ok(full)
}

pub fn read(base: &Path, path: &str, limit: u64) -> Result<Vec<u8>, String> {
    let path = resolve(base, path)?;
    let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(format!(
            "Evidence is not a file or exceeds {limit} bytes: {}",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Evidence grew beyond its read budget".into());
    }
    Ok(bytes)
}

pub fn verify(base: &Path, reference: &FileRef, limit: u64) -> Result<Vec<u8>, String> {
    if reference.sha256.len() != 64 || !reference.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("Invalid SHA-256 for {}", reference.path));
    }
    let bytes = read(base, &reference.path, limit)?;
    if digest(&bytes) != reference.sha256.to_lowercase() {
        return Err(format!("Stale evidence hash: {}", reference.path));
    }
    Ok(bytes)
}

pub fn git(base: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = std::process::Command::new("git");
    command
        .args(["-c", "core.fsmonitor=false", "-C"])
        .arg(base)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Git unavailable: {e}"))?;
    let out = pipe_reader(child.stdout.take().ok_or("Missing Git stdout")?, 65_536);
    let err = pipe_reader(child.stderr.take().ok_or("Missing Git stderr")?, 16_384);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Git inspection failed or exceeded five seconds: {result:?}"
                ));
            }
        }
    };
    let stdout = out.join().map_err(|_| "Git stdout reader panicked")??;
    let stderr = err.join().map_err(|_| "Git stderr reader panicked")??;
    if !status.success() || stdout.len() > 65_536 || stderr.len() > 16_384 {
        return Err(format!(
            "Git inspection failed or inventory exceeds its output budget: {}",
            String::from_utf8_lossy(&stderr)
        ));
    }
    Ok(stdout)
}

fn pipe_reader(
    reader: impl Read + Send + 'static,
    limit: u64,
) -> std::thread::JoinHandle<Result<Vec<u8>, String>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        reader
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    })
}

pub fn dirty_paths(base: &Path) -> Result<Vec<(String, bool)>, String> {
    let bytes = git(
        base,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut entries = bytes.split(|b| *b == 0).filter(|part| !part.is_empty());
    let mut paths = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            return Err("Malformed Git dirty inventory".into());
        }
        let renamed = entry[..2].contains(&b'R');
        let copied = entry[..2].contains(&b'C');
        let deleted = entry[..2].contains(&b'D');
        paths.push((
            String::from_utf8(entry[3..].to_vec())
                .map_err(|_| "Non-UTF8 dirty paths are unsupported")?,
            deleted,
        ));
        if renamed || copied {
            let original = entries.next().ok_or("Missing rename/copy source path")?;
            if renamed {
                paths.push((
                    String::from_utf8(original.to_vec()).map_err(|_| "Non-UTF8 rename source")?,
                    true,
                ));
            }
        }
    }
    if paths.len() > 256 {
        return Err(
            "Dirty inventory exceeds 256 paths; keep the current session recoverable".into(),
        );
    }
    paths.sort();
    Ok(paths)
}

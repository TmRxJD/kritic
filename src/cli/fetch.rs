//! Downloads for `kritic setup`: resumable, checksum-verified, with progress on stderr; plus
//! the release lookups that give each file its URL and expected SHA-256.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("kritic/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("http client")
}

/// A file to fetch: where from, its size and SHA-256 when the source publishes them.
#[derive(Debug, Clone)]
pub struct Remote {
    pub name: String,
    pub url: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Download `remote` to `dest`. An existing `dest` with the right checksum is kept; a partial
/// `.part` file is resumed. The file only appears at `dest` once its checksum matches.
pub async fn download(http: &reqwest::Client, remote: &Remote, dest: &Path) -> Result<(), String> {
    if dest.is_file() {
        match &remote.sha256 {
            Some(want) if sha256_file(dest).map_err(|e| e.to_string())? == *want => return Ok(()),
            Some(_) => {}
            None if remote.size.is_none_or(|s| dest.metadata().map(|m| m.len() == s).unwrap_or(false)) => return Ok(()),
            None => {}
        }
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let part = dest.with_extension(format!("{}part", dest.extension().map(|e| format!("{}.", e.to_string_lossy())).unwrap_or_default()));
    let mut have = part.metadata().map(|m| m.len()).unwrap_or(0);
    if remote.size.is_some_and(|s| have > s) {
        have = 0;
    }
    let mut req = http.get(&remote.url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| format!("{}: {e}", remote.name))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("{}: HTTP {status} from {}", remote.name, remote.url));
    }
    // A server that ignores Range sends the whole file again.
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    if !resumed {
        have = 0;
    }
    let total = remote.size.or_else(|| resp.content_length().map(|l| l + have));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&part)
        .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    let mut got = have;
    let mut stream = resp.bytes_stream();
    let t0 = Instant::now();
    let mut last = Instant::now();
    let tty = std::io::stderr().is_terminal();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("{}: download interrupted ({e}); run setup again to resume", remote.name))?;
        file.write_all(&chunk).map_err(|e| format!("cannot write {}: {e}", part.display()))?;
        got += chunk.len() as u64;
        if last.elapsed() >= Duration::from_millis(if tty { 250 } else { 5000 }) {
            last = Instant::now();
            let rate = (got - have) as f64 / t0.elapsed().as_secs_f64().max(0.001);
            let of = total.map(|t| format!(" / {}", super::ui::bytes(t))).unwrap_or_default();
            let line = format!("     {}  {}{of}  {}/s", remote.name, super::ui::bytes(got), super::ui::bytes(rate as u64));
            if tty {
                eprint!("\r{line:<78}");
            } else {
                eprintln!("{line}");
            }
        }
    }
    if tty {
        eprint!("\r{:<78}\r", "");
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    if let Some(want) = &remote.sha256 {
        let got = sha256_file(&part).map_err(|e| e.to_string())?;
        if got != *want {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{}: checksum mismatch (expected {want}, got {got}); the partial file was removed, run setup again", remote.name));
        }
    }
    std::fs::rename(&part, dest).map_err(|e| format!("cannot move {} into place: {e}", dest.display()))?;
    Ok(())
}

/// Extract entries of a .zip whose names `pick` maps to a relative output path.
pub fn extract_zip(archive: &Path, out: &Path, pick: impl Fn(&str) -> Option<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut written = Vec::new();
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let Some(rel) = pick(entry.name()) else { continue };
        let target = safe_join(out, &rel)?;
        if let Some(d) = target.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut w).map_err(|e| e.to_string())?;
        written.push(target);
    }
    Ok(written)
}

/// Extract a .tar.gz, keeping every regular file at its archive path.
pub fn extract_tar_gz(archive: &Path, out: &Path) -> Result<Vec<PathBuf>, String> {
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut t = tar::Archive::new(flate2::read::GzDecoder::new(f));
    let mut written = Vec::new();
    for entry in t.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let rel = entry.path().map_err(|e| e.to_string())?.into_owned();
        let target = safe_join(out, &rel)?;
        if let Some(d) = target.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        entry.unpack(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        written.push(target);
    }
    Ok(written)
}

/// Join without letting an archive entry escape `out` (`..`, absolute paths).
fn safe_join(out: &Path, rel: &Path) -> Result<PathBuf, String> {
    use std::path::Component;
    if rel.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(format!("refusing archive entry outside the target folder: {}", rel.display()));
    }
    Ok(out.join(rel))
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<Value, String> {
    let r = http.get(url).header("accept", "application/json").send().await.map_err(|e| format!("{url}: {e}"))?;
    let status = r.status();
    if !status.is_success() {
        return Err(format!("{url}: HTTP {status}"));
    }
    r.json().await.map_err(|e| format!("{url}: {e}"))
}

/// The latest GitHub release's asset `name` (by exact name or predicate), with its published digest.
pub async fn github_asset(http: &reqwest::Client, repo: &str, pick: impl Fn(&str) -> bool) -> Result<Remote, String> {
    let v = get_json(http, &format!("https://api.github.com/repos/{repo}/releases/latest")).await?;
    let assets = v["assets"].as_array().ok_or_else(|| format!("{repo}: release has no assets"))?;
    let a = assets
        .iter()
        .find(|a| a["name"].as_str().is_some_and(&pick))
        .ok_or_else(|| format!("{repo} {}: no matching asset for this platform", v["tag_name"].as_str().unwrap_or("?")))?;
    Ok(Remote {
        name: a["name"].as_str().unwrap_or_default().to_string(),
        url: a["browser_download_url"].as_str().unwrap_or_default().to_string(),
        sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(String::from),
        size: a["size"].as_u64(),
    })
}

/// A file in a Hugging Face model repo, with the SHA-256 from its LFS pointer.
pub async fn hf_file(http: &reqwest::Client, repo: &str, file: &str) -> Result<Remote, String> {
    let v = get_json(http, &format!("https://huggingface.co/api/models/{repo}/tree/main")).await?;
    let entry = v.as_array().and_then(|a| a.iter().find(|e| e["path"] == file)).ok_or_else(|| format!("{repo}: no file {file}"))?;
    Ok(Remote {
        name: file.to_string(),
        url: format!("https://huggingface.co/{repo}/resolve/main/{file}"),
        sha256: entry["lfs"]["oid"].as_str().map(String::from),
        size: entry["lfs"]["size"].as_u64().or(entry["size"].as_u64()),
    })
}

/// The newest wheel of `package` whose version starts with `version_prefix` for `platform_tag`.
pub async fn pypi_wheel(http: &reqwest::Client, package: &str, version_prefix: &str, platform_tag: &str) -> Result<Remote, String> {
    let v = get_json(http, &format!("https://pypi.org/pypi/{package}/json")).await?;
    let releases = v["releases"].as_object().ok_or_else(|| format!("{package}: no releases"))?;
    let mut versions: Vec<&String> = releases.keys().filter(|k| k.starts_with(version_prefix)).collect();
    versions.sort_by_key(|k| k.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>());
    for ver in versions.into_iter().rev() {
        if let Some(f) = releases[ver].as_array().and_then(|files| files.iter().find(|f| f["filename"].as_str().is_some_and(|n| n.ends_with(".whl") && n.contains(platform_tag)))) {
            return Ok(Remote {
                name: f["filename"].as_str().unwrap_or_default().to_string(),
                url: f["url"].as_str().unwrap_or_default().to_string(),
                sha256: f["digests"]["sha256"].as_str().map(String::from),
                size: f["size"].as_u64(),
            });
        }
    }
    Err(format!("{package} {version_prefix}*: no {platform_tag} wheel"))
}

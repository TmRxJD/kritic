//! A compact, read-only snapshot of a local checkout and an optional GitHub PR.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
    process::Stdio,
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_METADATA: usize = 256 * 1024;
const MAX_LOG: usize = 512 * 1024;
const MAX_EXCERPT: usize = 1_600;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitCiParams {
    /// Existing, trusted local Git checkout to inspect.
    pub cwd: String,
    /// When supplied, query GitHub via the signed-in gh CLI.
    pub pull_request: Option<PullRequest>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PullRequest {
    /// GitHub.com owner/repository.
    pub repo: String,
    /// Pull request number.
    #[schemars(range(min = 1))]
    pub number: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GitCiSnapshot {
    pub git: GitSnapshot,
    pub ci: Option<CiSnapshot>,
    pub ci_error: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GitSnapshot {
    pub head: Option<String>,
    pub branch: String,
    /// Porcelain status records. Untracked directories can represent multiple files.
    pub status_records: usize,
    /// The status query excluded submodules. Zero records leaves their status unknown.
    pub submodules_excluded: bool,
    /// Configured clean/process filters were disabled. Results may differ from ordinary Git status.
    pub filters_disabled: bool,
    /// Up to eight records as XY plus Git-escaped path (including original rename path).
    /// Untracked and ignored records retain their `?` and `!` markers.
    pub paths_sample: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CiSnapshot {
    pub head: String,
    pub url: String,
    pub checks: BTreeMap<String, usize>,
    pub first_failure: Option<Failure>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Failure {
    pub name: String,
    pub state: String,
    pub url: Option<String>,
    pub excerpt: Option<String>,
    pub log_prefix_truncated: bool,
    pub log_unavailable: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhSnapshot {
    head_ref_oid: String,
    url: String,
    status_check_rollup: Vec<Check>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Check {
    name: Option<String>,
    context: Option<String>,
    conclusion: Option<String>,
    state: Option<String>,
    status: Option<String>,
    details_url: Option<String>,
    target_url: Option<String>,
}

impl PullRequest {
    fn validate(&self) -> Result<(), String> {
        let valid = |part: &str| {
            !part.is_empty()
                && part.len() <= 100
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        if self.number == 0
            || !self.repo.split_once('/').is_some_and(|(owner, repo)| {
                valid(owner) && owner.as_bytes()[0].is_ascii_alphanumeric() && valid(repo)
            })
        {
            return Err("pull_request requires owner/repository and a positive number".into());
        }
        Ok(())
    }
}

impl Check {
    fn state(&self) -> &'static str {
        match self
            .conclusion
            .as_deref()
            .filter(|state| !state.is_empty())
            .or(self.state.as_deref())
            .or(self.status.as_deref())
        {
            Some("SUCCESS") => "success",
            Some("FAILURE" | "ERROR" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED") => {
                "failure"
            }
            Some("CANCELLED") => "cancelled",
            Some("NEUTRAL" | "SKIPPED") => "skipped_or_neutral",
            Some("EXPECTED" | "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED") => {
                "pending"
            }
            _ => "unknown",
        }
    }
}

fn github_job_id(repo: &str, url: &str) -> Option<u64> {
    let url = reqwest::Url::parse(url).ok()?;
    if url.scheme() != "https"
        || url.host_str()? != "github.com"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let [owner, name, "actions", "runs", run, "job", job] =
        url.path_segments()?.collect::<Vec<_>>()[..]
    else {
        return None;
    };
    let (expected_owner, expected_name) = repo.split_once('/')?;
    if !owner.eq_ignore_ascii_case(expected_owner)
        || !name.eq_ignore_ascii_case(expected_name)
        || run.parse::<u64>().ok()? == 0
    {
        return None;
    }
    job.parse::<u64>().ok().filter(|id| *id > 0)
}

fn bounded_text(s: &str, max: usize) -> String {
    s.chars()
        .filter(|&c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(max)
        .collect()
}

fn log_excerpt(log: &str) -> String {
    let lines = log.lines().collect::<Vec<_>>();
    let marked = lines.iter().position(|line| {
        let line = line.to_ascii_lowercase();
        line.contains("##[error]") || line.contains("::error")
    });
    let diagnostic = marked.or_else(|| {
        lines.iter().position(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("error:") || line.contains("failed")
        })
    });
    let Some(begin) = diagnostic else {
        return bounded_text(
            &lines[lines.len().saturating_sub(6)..].join("\n"),
            MAX_EXCERPT,
        );
    };
    let mut excerpt = String::new();
    if begin > 0 {
        excerpt.push_str(&bounded_text(lines[begin - 1], 200));
        excerpt.push('\n');
    }
    let remaining = MAX_EXCERPT - excerpt.chars().count();
    excerpt.push_str(&bounded_text(&lines[begin..].join("\n"), remaining));
    excerpt
}

struct Capture {
    text: String,
    truncated: bool,
}

async fn capture(mut reader: impl AsyncRead + Unpin, cap: usize) -> io::Result<Capture> {
    let mut prefix = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let keep = count.min(cap.saturating_sub(prefix.len()));
        prefix.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    Ok(Capture {
        text: String::from_utf8_lossy(&prefix).into_owned(),
        truncated,
    })
}

async fn read_command(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    cap: usize,
) -> Result<Capture, String> {
    // Resolve the fixed executable before selecting the checkout as its working directory.
    let file = if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_owned()
    };
    let resolved = crate::clirun::resolve(&file)
        .filter(|path| {
            path.is_absolute()
                && path.file_name().and_then(std::ffi::OsStr::to_str) == Some(file.as_str())
        })
        .ok_or_else(|| format!("{file} was not found on an absolute PATH entry"))?;
    let mut cmd = tokio::process::Command::from(crate::proc::hidden_command(resolved));
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "")
        .env_remove("GH_DEBUG");
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd)
            .env_remove("GIT_DIR")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
    }
    let mut child = cmd.spawn().map_err(|e| format!("start {program}: {e}"))?;
    let stdout = child.stdout.take().ok_or("missing stdout pipe")?;
    let stderr = child.stderr.take().ok_or("missing stderr pipe")?;
    let (status, stdout, stderr) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::try_join!(child.wait(), capture(stdout, cap), capture(stderr, 1_000))
    })
    .await
    .map_err(|_| format!("{program} timed out after 15 seconds"))?
    .map_err(|e| format!("read {program}: {e}"))?;
    if status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "{program} exited with {status}: {}",
            bounded_text(stderr.text.trim(), 1_000)
        ))
    }
}

fn sampled_path(line: &str) -> Option<String> {
    let (kind, rest) = line.split_once(' ')?;
    let fields = match kind {
        "1" => 8,
        "2" => 9,
        "u" => 10,
        "?" | "!" => return Some(bounded_text(line, 200)),
        _ => return None,
    };
    // The record prefix is fixed-width in fields. The last field is Git's path
    // representation and can contain spaces or a tab separating rename paths.
    let mut rest = rest.splitn(fields, ' ');
    let status = rest.next()?;
    let path = rest.nth(fields - 2)?;
    Some(bounded_text(&format!("{status} {path}"), 200))
}

fn git_status(output: &str, filters_disabled: bool) -> Result<GitSnapshot, String> {
    let oid = output
        .lines()
        .find_map(|line| line.strip_prefix("# branch.oid "))
        .ok_or("git status did not report the HEAD")?;
    let branch = output
        .lines()
        .find_map(|line| line.strip_prefix("# branch.head "))
        .ok_or("git status did not report the branch")?;
    let records = output.lines().filter(|line| !line.starts_with('#'));
    Ok(GitSnapshot {
        head: (oid != "(initial)").then(|| oid.to_owned()),
        branch: bounded_text(branch, 200),
        status_records: records.clone().count(),
        submodules_excluded: true,
        filters_disabled,
        paths_sample: records.filter_map(sampled_path).take(8).collect(),
    })
}

fn filter_overrides(config_names: &str) -> Result<Vec<String>, String> {
    let mut drivers = BTreeSet::new();
    for key in config_names.split('\0') {
        let Some((name, "clean" | "process")) = key
            .strip_prefix("filter.")
            .and_then(|key| key.rsplit_once('.'))
        else {
            continue;
        };
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err("Git clean/process filter name cannot be safely overridden".into());
        }
        drivers.insert(name);
        if drivers.len() > 32 {
            return Err("Git has more than 32 configured clean/process filters".into());
        }
    }
    Ok(drivers
        .into_iter()
        .flat_map(|name| {
            [
                "-c".into(),
                format!("filter.{name}.clean="),
                "-c".into(),
                format!("filter.{name}.process="),
                "-c".into(),
                format!("filter.{name}.required=false"),
            ]
        })
        .collect())
}

async fn local_snapshot(cwd: &Path) -> Result<GitSnapshot, String> {
    // Load only effective config keys, so external filter programs are neither
    // executed nor copied into evidence. Includes can declare active filters.
    let config = read_command(
        "git",
        &[
            "--no-optional-locks",
            "config",
            "--list",
            "--null",
            "--name-only",
            "--includes",
        ],
        Some(cwd),
        MAX_METADATA,
    )
    .await?;
    if config.truncated {
        return Err("Git config names exceeded the 256 KiB filter inspection limit".into());
    }
    let overrides = filter_overrides(&config.text)?;
    let mut args = vec!["--no-optional-locks", "-c", "core.fsmonitor=false"];
    args.extend(overrides.iter().map(String::as_str));
    args.extend([
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=normal",
        "--ignore-submodules=all",
    ]);
    let raw = read_command("git", &args, Some(cwd), MAX_METADATA).await?;
    if raw.truncated {
        return Err("Git status exceeded the 256 KiB inspection limit".into());
    }
    git_status(&raw.text, !overrides.is_empty())
}

async fn github_snapshot(pr: PullRequest) -> Result<CiSnapshot, String> {
    let repo = format!("github.com/{}", pr.repo);
    let number = pr.number.to_string();
    let raw = read_command(
        "gh",
        &[
            "pr",
            "view",
            &number,
            "--repo",
            &repo,
            "--json",
            "headRefOid,url,statusCheckRollup",
        ],
        None,
        MAX_METADATA,
    )
    .await?;
    if raw.truncated {
        return Err("GitHub check rollup exceeded the 256 KiB inspection limit".into());
    }
    let raw: GhSnapshot =
        serde_json::from_str(&raw.text).map_err(|e| format!("GitHub check rollup: {e}"))?;
    if raw.head_ref_oid.len() != 40 || !raw.head_ref_oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("GitHub returned an invalid PR head SHA".into());
    }
    let mut checks = BTreeMap::new();
    let mut first = None;
    for check in raw.status_check_rollup {
        let state = check.state();
        *checks.entry(state.to_owned()).or_insert(0) += 1;
        if first.is_none() && matches!(state, "failure" | "cancelled") {
            first = Some(Failure {
                name: bounded_text(
                    check
                        .name
                        .or(check.context)
                        .as_deref()
                        .unwrap_or("unnamed check"),
                    200,
                ),
                state: state.to_owned(),
                url: check
                    .details_url
                    .or(check.target_url)
                    .filter(|url| url.len() <= 1_000),
                excerpt: None,
                log_prefix_truncated: false,
                log_unavailable: None,
            });
        }
    }
    if let Some(failure) = &mut first {
        if let Some(job) = failure
            .url
            .as_deref()
            .and_then(|url| github_job_id(&pr.repo, url))
        {
            let endpoint = format!("repos/{}/actions/jobs/{job}/logs", pr.repo);
            match read_command(
                "gh",
                &[
                    "api",
                    "--hostname",
                    "github.com",
                    "--method",
                    "GET",
                    &endpoint,
                ],
                None,
                MAX_LOG,
            )
            .await
            {
                Ok(log) => {
                    failure.excerpt = Some(log_excerpt(&log.text));
                    failure.log_prefix_truncated = log.truncated;
                }
                Err(error) => failure.log_unavailable = Some(error),
            }
        } else {
            failure.log_unavailable = Some("No same-repository GitHub Actions job URL".into());
        }
    }
    Ok(CiSnapshot {
        head: raw.head_ref_oid,
        url: bounded_text(&raw.url, 1_000),
        checks,
        first_failure: first,
    })
}

/// Inspect local status. A remote failure preserves the local result.
pub async fn snapshot(p: GitCiParams) -> Result<GitCiSnapshot, String> {
    if let Some(pr) = &p.pull_request {
        pr.validate()?;
    }
    let cwd = Path::new(&p.cwd);
    if p.cwd.is_empty() || p.cwd.len() > 4096 || !cwd.is_dir() {
        return Err("cwd must be an existing directory path up to 4096 bytes".into());
    }
    let git = local_snapshot(cwd).await?;
    let (ci, ci_error) = match p.pull_request {
        Some(pr) => match github_snapshot(pr).await {
            Ok(ci) => (Some(ci), None),
            Err(error) => (None, Some(error)),
        },
        None => (None, None),
    };
    Ok(GitCiSnapshot { git, ci, ci_error })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_job_download_is_scoped_to_expected_repo() {
        assert_eq!(
            github_job_id(
                "Org/Repo",
                "https://github.com/org/repo/actions/runs/1/job/2"
            ),
            Some(2)
        );
        for url in [
            "https://github.com/org/other/actions/runs/1/job/2",
            "https://github.com.attacker.test/org/repo/actions/runs/1/job/2",
            "https://github.com/org/repo/actions/runs/1/job/2/extra",
        ] {
            assert_eq!(github_job_id("Org/Repo", url), None);
        }
    }

    #[test]
    fn repository_name_cannot_alter_api_path() {
        for repo in ["org/../repo", "--repo/org", "org/%2e%2e", "org/repo?other"] {
            assert!(
                PullRequest {
                    repo: repo.into(),
                    number: 1
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn local_status_reports_detached_and_unborn_heads() {
        let git = git_status("# branch.oid (initial)\n# branch.head main\n? a\n", false).unwrap();
        assert_eq!(
            (git.head, git.branch.as_str(), git.status_records),
            (None, "main", 1)
        );
        let git = git_status("# branch.oid 123\n# branch.head (detached)\n", false).unwrap();
        assert_eq!(
            (git.head.as_deref(), git.branch.as_str(), git.status_records),
            (Some("123"), "(detached)", 0)
        );
    }

    #[test]
    fn paths_sample_retains_status_and_git_escaped_paths_without_modes_or_object_ids() {
        let git = git_status(
            concat!(
                "# branch.oid 123\n# branch.head main\n",
                "1 .M N... 100644 100644 100644 hash1 hash2 \"file \\303\\251.txt\"\n",
                "2 R. N... 100644 100644 100644 hash1 hash2 R100 new path\told path\n",
                "u UU N... 100644 100644 100644 100644 hash1 hash2 hash3 conflict\n",
                "? untracked directory/\n",
            ),
            true,
        )
        .unwrap();
        assert!(git.submodules_excluded);
        assert!(git.filters_disabled);
        assert_eq!(git.status_records, 4);
        assert_eq!(
            git.paths_sample,
            [
                ".M \"file \\303\\251.txt\"",
                "R. new path\told path",
                "UU conflict",
                "? untracked directory/"
            ]
        );
    }

    #[test]
    fn configured_filters_are_neutralized_by_name_or_rejected() {
        let overrides =
            filter_overrides("filter.lfs.clean\0filter.lfs.process\0filter.other.smudge\0")
                .unwrap();
        assert_eq!(
            overrides,
            [
                "-c",
                "filter.lfs.clean=",
                "-c",
                "filter.lfs.process=",
                "-c",
                "filter.lfs.required=false"
            ]
        );
        assert!(filter_overrides("filter.unsupported name.clean\0").is_err());
    }

    #[test]
    fn excerpt_includes_failure_and_is_bounded() {
        let log = format!("intro\ncontext\n##[error]failed\n{}", "x".repeat(10_000));
        let excerpt = log_excerpt(&log);
        assert!(excerpt.starts_with("context\n##[error]failed\n"));
        assert_eq!(excerpt.chars().count(), MAX_EXCERPT);
    }
}

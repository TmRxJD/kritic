//! Deterministic checks: parsers and real project commands. These are facts, not model opinions;
//! any failure here forces approval=false.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    Skipped,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Check {
    pub check: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Check {
    fn pass(name: &str) -> Self {
        Check {
            check: name.into(),
            status: Status::Pass,
            detail: None,
        }
    }
    fn fail(name: &str, detail: impl Into<String>) -> Self {
        Check {
            check: name.into(),
            status: Status::Fail,
            detail: Some(clip(&detail.into(), 300)),
        }
    }
    fn skipped(name: &str, detail: &str) -> Self {
        Check {
            check: name.into(),
            status: Status::Skipped,
            detail: Some(detail.into()),
        }
    }
}

pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let head: String = s.chars().take(n).collect();
    format!("{head}...")
}

fn tail(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_string();
    }
    let t: String = s.chars().skip(count - n).collect();
    format!("...{t}")
}

pub async fn deterministic_checks(content: &str, kind: &str) -> Vec<Check> {
    let mut out = Vec::new();
    match kind {
        "json" | "map" => out.push(match serde_json::from_str::<serde_json::Value>(content) {
            Ok(_) => Check::pass("json_parse"),
            Err(e) => Check::fail("json_parse", e.to_string()),
        }),
        "yaml" => {
            let parsed: Result<Vec<serde_yaml::Value>, _> =
                serde_yaml::Deserializer::from_str(content)
                    .map(serde::Deserialize::deserialize)
                    .collect();
            out.push(match parsed {
                Ok(_) => Check::pass("yaml_parse"),
                Err(e) => Check::fail("yaml_parse", e.to_string()),
            });
            if content.contains('\t') {
                out.push(Check::fail("yaml_no_tabs", "tab characters in YAML"));
            }
        }
        "toml" => out.push(match content.parse::<toml::Table>() {
            Ok(_) => Check::pass("toml_parse"),
            Err(e) => Check::fail("toml_parse", e.to_string()),
        }),
        "python" => out.push(python_syntax(content).await),
        _ => {}
    }
    if content
        .lines()
        .any(|l| l.starts_with("<<<<<<< ") || l.starts_with(">>>>>>> "))
    {
        out.push(Check::fail(
            "no_merge_conflict_markers",
            "merge conflict markers present",
        ));
    } else {
        out.push(Check::pass("no_merge_conflict_markers"));
    }
    if content.contains('\u{FFFD}') {
        out.push(Check::fail("utf8_clean", "invalid UTF-8 bytes"));
    } else {
        out.push(Check::pass("utf8_clean"));
    }
    out
}

async fn python_syntax(content: &str) -> Check {
    let child = tokio::process::Command::new("python")
        .args([
            "-c",
            "import ast,sys; ast.parse(sys.stdin.buffer.read().decode('utf-8','replace'))",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let Ok(mut child) = child else {
        return Check::skipped("python_syntax", "python not on PATH");
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(content.as_bytes()).await;
    }
    match tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await {
        Ok(Ok(o)) if o.status.success() => Check::pass("python_syntax"),
        Ok(Ok(o)) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let last = err
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("syntax error");
            Check::fail("python_syntax", last)
        }
        _ => Check::skipped("python_syntax", "python check timed out"),
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CommandResult {
    pub command: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// Last ~4,000 characters of combined stdout+stderr, verbatim.
    pub output_tail: String,
}

/// Run the project's own validators (tsc, lint, tests...) exactly as given. Output is reported
/// verbatim; nothing is summarized or inferred.
pub async fn run_command(cmd: &str, dir: &Path, timeout_s: u64) -> CommandResult {
    let t0 = Instant::now();
    if cmd.trim().is_empty() {
        return CommandResult {
            command: cmd.into(),
            status: Status::Skipped,
            exit_code: None,
            duration_ms: 0,
            output_tail: "empty command; nothing executed".into(),
        };
    }
    #[cfg(windows)]
    let mut c = {
        use std::os::windows::process::CommandExt;
        let mut c = tokio::process::Command::new("cmd");
        // cmd.exe does not use C-runtime argument escaping. Preserve the caller's
        // shell command inside cmd's outer quote pair, including quoted paths.
        c.args(["/D", "/S", "/C"]);
        c.as_std_mut().raw_arg(format!("\"{cmd}\""));
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    c.current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let res = match c.spawn() {
        Err(e) => Err(format!("failed to start: {e}")),
        Ok(child) => {
            match tokio::time::timeout(Duration::from_secs(timeout_s), child.wait_with_output())
                .await
            {
                Err(_) => Err(format!("timed out after {timeout_s}s")),
                Ok(Err(e)) => Err(e.to_string()),
                Ok(Ok(o)) => Ok(o),
            }
        }
    };
    let duration_ms = t0.elapsed().as_millis() as u64;
    match res {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            CommandResult {
                command: cmd.into(),
                status: if o.status.success() {
                    Status::Pass
                } else {
                    Status::Fail
                },
                exit_code: o.status.code(),
                duration_ms,
                output_tail: tail(&text, 4000),
            }
        }
        Err(e) => CommandResult {
            command: cmd.into(),
            status: Status::Fail,
            exit_code: None,
            duration_ms,
            output_tail: e,
        },
    }
}

//! Run a harness process: JSON in on stdin, one JSON object out on stdout, progress events as
//! JSON lines on stderr (`{"event": ...}`) streamed to the WebSocket bus while it runs.

use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Noninteractive helpers must not create console windows when launched from a desktop MCP host.
pub fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW; GUI apps retain their own windows.
    }
    command
}

pub struct Output {
    pub stdout: String,
    /// Non-event stderr lines (last ~40), for error messages.
    pub stderr_tail: String,
}

pub async fn run_streaming(
    mut cmd: tokio::process::Command,
    input: &Value,
    timeout_s: u64,
    tool: &str,
) -> Result<Output, String> {
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start harness: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.to_string().as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        drop(stdin);
    }
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let err_task = relay_stderr(stderr, tool);
    let run = async {
        let mut out = String::new();
        stdout
            .read_to_string(&mut out)
            .await
            .map_err(|e| e.to_string())?;
        child.wait().await.map_err(|e| e.to_string())?;
        Ok::<String, String>(out)
    };
    let out = tokio::time::timeout(Duration::from_secs(timeout_s), run)
        .await
        .map_err(|_| format!("harness timed out after {timeout_s}s"))??;
    let stderr_tail = err_task.await.unwrap_or_default();
    Ok(Output {
        stdout: out,
        stderr_tail,
    })
}

/// Stream a harness's stderr: `{"event": ...}` lines go to the event bus as `progress` from
/// `tool`; the task returns the last ~40 other lines, for error messages.
pub fn relay_stderr(
    stderr: tokio::process::ChildStderr,
    tool: &str,
) -> tokio::task::JoinHandle<String> {
    let tool = tool.to_string();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: Vec<String> = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<Value>(&line) {
                Ok(v) if v.get("event").is_some() => {
                    let mut ev = v;
                    ev["tool"] = json!(tool);
                    crate::events::emit("progress", ev);
                }
                _ => {
                    tail.push(line);
                    if tail.len() > 40 {
                        tail.remove(0);
                    }
                }
            }
        }
        tail.join(
            "
",
        )
    })
}

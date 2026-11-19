//! Bounded prompt generation through the explicitly selected official Codex CLI.
use std::path::Path;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::SYSTEM;

/// Generate a prompt proposal in a restricted read-only Codex turn.
///
/// Returns an error on inventory, transport, completion or JSON decoding failure.
pub async fn generate(executable: &Path, model: &str, packet: &str) -> Result<Value, String> {
    let disables = codex_mcp_disables(executable).await?;
    let mut command = tokio::process::Command::new(executable);
    detach_parent_tools(&mut command);
    command.args([
        "--disable",
        "shell_tool",
        "--disable",
        "unified_exec",
        "--disable",
        "apps",
        "--disable",
        "plugins",
        "--disable",
        "multi_agent",
        "--disable",
        "multi_agent_v2",
        "--disable",
        "in_app_browser",
        "-c",
        "web_search=\"disabled\"",
    ]);
    for setting in disables {
        command.arg("-c").arg(setting);
    }
    command
        .args([
            "-a",
            "never",
            "exec",
            "--ephemeral",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--json",
        ])
        .env("KRITIC_PROMPT_INTERNAL", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if !model.is_empty() {
        command.arg("--model").arg(model);
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.arg("-").spawn().map_err(
        |_| "Cannot launch official Codex CLI; set prompt.codex_exe and run its login flow",
    )?;
    let instruction = format!("[kritic-prompt-review]\n{SYSTEM}\n{packet}");
    let mut stdin = child.stdin.take().ok_or("Codex prompt stdin unavailable")?;
    stdin
        .write_all(instruction.as_bytes())
        .await
        .map_err(|_| "Cannot send prompt to Codex")?;
    drop(stdin);
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or("Codex prompt stdout unavailable")?
        .take(262145)
        .read_to_end(&mut output)
        .await
        .map_err(|_| "Cannot read Codex prompt output")?;
    if output.len() > 262144 {
        return Err("Codex prompt output exceeds 256 KiB".into());
    }
    if !child
        .wait()
        .await
        .map_err(|_| "Cannot wait for Codex prompt review")?
        .success()
    {
        return Err(
            "Codex prompt review failed; check official CLI login status and account access".into(),
        );
    }
    decode_codex(&output)
}

async fn codex_mcp_disables(executable: &Path) -> Result<Vec<String>, String> {
    // An empty mcp_servers table merges with existing servers; it does NOT disable
    // them. Ask the official CLI for names, then explicitly disable every server.
    // Do not persist or log the returned configuration, which may contain secrets.
    let mut command = tokio::process::Command::new(executable);
    detach_parent_tools(&mut command);
    command
        .args([
            "--disable",
            "apps",
            "--disable",
            "plugins",
            "--disable",
            "in_app_browser",
            "mcp",
            "list",
            "--json",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command
        .spawn()
        .map_err(|_| "Cannot inspect official Codex MCP configuration")?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .ok_or("Codex MCP inventory stdout unavailable")?
        .take(131073)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Cannot read Codex MCP inventory")?;
    if bytes.len() > 131072
        || !child
            .wait()
            .await
            .map_err(|_| "Cannot wait for Codex MCP inventory")?
            .success()
    {
        return Err(
            "Codex MCP inventory failed or exceeded its bound; prompt review not started".into(),
        );
    }
    let inventory: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid Codex MCP inventory JSON")?;
    let servers = inventory
        .as_array()
        .ok_or("Codex MCP inventory must be an array")?;
    if servers.len() > 128 {
        return Err("Too many Codex MCP servers for bounded prompt review".into());
    }
    servers
        .iter()
        .filter(|server| server["enabled"] == true)
        .map(|server| {
            let name = server["name"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 128)
                .ok_or("Invalid Codex MCP server name")?;
            // Codex's -c path parser does not parse TOML quoted path segments:
            // quotes would create a different server instead of disabling it.
            if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(
                    "Codex MCP server name cannot be safely overridden; prompt review not started"
                        .into(),
                );
            }
            Ok(format!("mcp_servers.{name}.enabled=false"))
        })
        .collect()
}

fn detach_parent_tools(command: &mut tokio::process::Command) {
    // A child reviewer must not attach to the parent's desktop tool bridge or
    // advertise its session identity. User configuration, auth and safeguards stay.
    for name in [
        "CODEX_APP_TOOLS_PIPE_PATH",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "CODEX_TOOL_CALL_ID",
    ] {
        command.env_remove(name);
    }
}

/// Require a completed turn and one final message; a running or failed turn is not evidence.
pub fn decode_codex(output: &[u8]) -> Result<Value, String> {
    let mut final_text = None;
    let mut completed = false;
    for line in output.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let event: Value = serde_json::from_slice(line).map_err(|_| "Invalid Codex event JSON")?;
        if event["type"] == "turn.failed" || event["type"] == "error" {
            return Err("Codex prompt turn failed; original retained".into());
        }
        if event["type"] == "turn.completed" {
            completed = true;
        }
        if event["type"] == "item.completed" {
            match event["item"]["type"].as_str() {
                Some("agent_message") => {
                    final_text = event["item"]["text"].as_str().map(str::to_owned)
                }
                Some("reasoning" | "error") => {}
                _ => return Err(
                    "Codex prompt reviewer attempted a tool or unsupported item; original retained"
                        .into(),
                ),
            }
        }
    }
    if !completed {
        return Err("Codex prompt turn did not complete".into());
    }
    serde_json::from_str(
        final_text
            .as_deref()
            .ok_or("Codex prompt review omitted final text")?,
    )
    .map_err(|_| "Codex prompt proposal is not JSON".into())
}

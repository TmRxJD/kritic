//! Explicit prompt intake and nonblocking native-host augmentation.
use clap::Args;
use serde_json::{Value, json};
use std::io::{Read, Write};

#[derive(Debug, Args)]
pub struct PromptArgs {
    /// Print the proposed-brief response schema, without model inference.
    #[arg(long)]
    schema: bool,
    /// Read a native UserPromptSubmit JSON event, then emit additionalContext.
    #[arg(long,value_parser=["codex","claude","hermes"])]
    hook: Option<String>,
    /// Save the full original/proposed report in a new permanent file.
    #[arg(long)]
    output: Option<std::path::PathBuf>,
    /// Start the official Codex login flow; Kritic never reads its credentials.
    #[arg(long,value_parser=["codex"],conflicts_with_all=["hook","schema","provider_status"])]
    login: Option<String>,
    /// Inspect selected provider setup without inference or printing credentials.
    #[arg(long,conflicts_with_all=["hook","schema","login"])]
    provider_status: bool,
}

pub async fn run(args: PromptArgs) -> Result<(), String> {
    if args.schema {
        println!("{}", crate::prompt::schema());
        return Ok(());
    }
    let cfg = crate::config::Config::load();
    if args.login.is_some() {
        let status = tokio::process::Command::new(&cfg.prompt_codex_exe)
            .arg("login")
            .status()
            .await
            .map_err(|_| "Cannot start official Codex login; configure prompt.codex_exe")?;
        return if status.success() {
            Ok(())
        } else {
            Err("Official Codex login did not complete".into())
        };
    }
    if args.provider_status {
        let mut command = tokio::process::Command::new(&cfg.prompt_codex_exe);
        command
            .args(["login", "status"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let status =
            tokio::time::timeout(std::time::Duration::from_secs(5), command.status()).await;
        let native_login = matches!(status,Ok(Ok(code)) if code.success());
        println!(
            "{}",
            json!({"backend":cfg.prompt_backend,"model":cfg.prompt_model,"enabled":cfg.tool_enabled("prompt_critic"),
            "native_codex_login_detected":native_login,"cloud_key_configured":cfg.cloud_key().is_some(),
            "inference_tested":false,"direct_chatgpt_oauth_implemented":false,"claude_subscription_relay_supported":false,
            "config_errors":cfg.config_errors})
        );
        return Ok(());
    }
    if args.hook.is_some() && std::env::var_os("KRITIC_PROMPT_INTERNAL").is_some() {
        println!("{{}}");
        return Ok(());
    }
    let mut input = String::new();
    let read = std::io::stdin().take(262145).read_to_string(&mut input);
    let result = async {
        read.map_err(|e| format!("Cannot read prompt input: {e}"))?;
        if input.len() > 262144 {
            return Err("Prompt event exceeds 256 KiB".into());
        }
        let original = if args.hook.is_some() {
            let event: Value =
                serde_json::from_str(&input).map_err(|_| "Invalid prompt hook event JSON")?;
            event["prompt"]
                .as_str()
                .ok_or("Hook event requires the original prompt string")?
                .to_owned()
        } else {
            input
        };
        let report = crate::prompt::review(&cfg, &original, args.hook.is_some()).await?;
        if let Some(path) = args.output {
            let bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
            // Permanent destination chosen by caller. Never overwrite prior evidence.
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| format!("Cannot create prompt report {}: {e}", path.display()))?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
        }
        Ok::<_, String>(report)
    }
    .await;
    if args.hook.is_some() {
        match result {
            Ok(report) => {
                if let Some(context) = crate::prompt::hook_context(&report) {
                    let output = if args.hook.as_deref() == Some("hermes") {
                        json!({"context":context})
                    } else {
                        json!({"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":context}})
                    };
                    println!("{output}");
                } else {
                    println!("{{}}");
                }
                if let Some(error) = report.get("provider_error") {
                    eprintln!("Kritic prompt review unavailable: {error}; original retained");
                }
            }
            Err(error) => {
                eprintln!("Kritic prompt hook: {error}; original retained");
                println!("{{}}");
            }
        }
        return Ok(());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&result?).map_err(|e| e.to_string())?
    );
    Ok(())
}

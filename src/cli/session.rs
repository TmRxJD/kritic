//! Inspect pinned session/handoff evidence without model inference or implicit session creation.
use clap::Args;

#[derive(Debug, Args)]
pub struct SessionArgs {
    #[arg(long, default_value = ".")]
    base_dir: String,
    #[arg(long)]
    case: Option<String>,
    #[arg(long)]
    handoff: bool,
    #[arg(long)]
    schema: bool,
    /// Save the full report to a new permanent path; never overwrite earlier evidence.
    #[arg(long)]
    output: Option<std::path::PathBuf>,
}

pub fn run(args: SessionArgs) -> Result<(), String> {
    if args.schema {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::continuity::schema()).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    let cfg = crate::config::Config::load();
    let name = if args.handoff {
        "handoff_critic"
    } else {
        "session_critic"
    };
    if !cfg.tool_enabled(name) {
        return Err("Enable capabilities.sessions=true or KRITIC_CAP_SESSIONS=true".into());
    }
    let request = crate::continuity::Request {
        base_dir: args.base_dir,
        case_path: args.case,
    };
    let mut report = if args.handoff {
        crate::continuity::handoff_critic(&request)?
    } else {
        crate::continuity::session_critic(&request)?
    };
    if let Some(path) = args.output {
        use std::io::Write;
        let bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(&bytes).map_err(|e| e.to_string())?;
                file.sync_all().map_err(|e| e.to_string())?;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && std::fs::read(&path).ok().as_deref() == Some(&bytes) => {}
            Err(error) => return Err(format!("Report output {}: {error}", path.display())),
        }
        report
            .as_object_mut()
            .ok_or("Expected object report")?
            .remove("curated_context");
        report["full_result"] =
            serde_json::json!({"path":path,"sha256":crate::continuity::evidence::digest(&bytes)});
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    Ok(())
}

//! Opt-in evidence consistency checkpoint, without execution or model inference.
use clap::Args;

#[derive(Debug, Args)]
pub struct FramingArgs {
    #[arg(long, default_value = ".")]
    base_dir: String,
    #[arg(long, conflicts_with = "schema")]
    case: Option<String>,
    /// Print the authoritative case JSON Schema without loading a case.
    #[arg(long)]
    schema: bool,
}

pub async fn run(args: FramingArgs) -> Result<(), String> {
    if args.schema {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::framing::schema()).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    let cfg = crate::config::Config::load();
    if !cfg.tool_enabled("framing_critic") {
        return Err(
            "framing disabled; set capabilities.framing=true or KRITIC_CAP_FRAMING=true".into(),
        );
    }
    let request = crate::framing::Request {
        base_dir: args.base_dir,
        case_path: args.case,
    };
    let mut report = tokio::task::spawn_blocking(move || crate::framing::execute(&request))
        .await
        .map_err(|e| format!("framing worker: {e}"))??;
    report.full_result = crate::tools::save_full(&cfg, "framing_critic", &report);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    Ok(())
}

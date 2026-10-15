//! Read-only dependency and maintenance review; network access is a separate opt-in.
use clap::Args;

#[derive(Debug, Args)]
pub struct DebtArgs {
    #[arg(long, default_value = ".")]
    base_dir: String,
    #[arg(long)]
    online: bool,
    #[arg(long, requires = "online")]
    refresh: bool,
    #[arg(long, num_args=1..)]
    source_paths: Vec<String>,
}

pub async fn run(args: DebtArgs) -> Result<(), String> {
    let cfg = crate::config::Config::load();
    let mut report = crate::debt::execute(
        &cfg,
        crate::debt::Request {
            base_dir: args.base_dir,
            online: args.online,
            refresh: args.refresh,
            source_paths: args.source_paths,
        },
    )
    .await?;
    report.full_result = crate::tools::save_full(&cfg, "technical_debt_critic", &report);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    Ok(())
}

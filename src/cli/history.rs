//! Explicit local review snapshots; no background memory or model startup.
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct HistoryArgs {
    #[arg(long, default_value = ".", global = true)]
    base_dir: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Store a saved JSON report with the exact input corpus and context fingerprint.
    Record {
        #[arg(long)]
        report: String,
        #[arg(long, required = true, num_args = 1..)]
        inputs: Vec<String>,
        #[arg(long)]
        context: String,
    },
    /// Read historical metadata (25 records per page); this never reruns checks.
    List {
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Compare two historical records with matching tool/context identities.
    Compare {
        #[arg(long)]
        left: String,
        #[arg(long)]
        right: String,
    },
    /// Save sanitized, low-trust jcode MemoryStore JSON; never modify live jcode data.
    ExportJcode,
}

pub fn run(args: HistoryArgs) -> Result<(), String> {
    use crate::history_api::Request;
    let base_dir = args.base_dir;
    let request = match args.command {
        Command::Record {
            report,
            inputs,
            context,
        } => Request::Record {
            base_dir,
            report,
            inputs,
            context,
        },
        Command::List { offset } => Request::List { base_dir, offset },
        Command::Compare { left, right } => Request::Compare {
            base_dir,
            left,
            right,
        },
        Command::ExportJcode => Request::ExportJcode { base_dir },
    };
    let result = crate::history_api::execute(&crate::config::Config::load(), request)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
    );
    Ok(())
}

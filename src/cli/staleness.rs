//! Offline test-currency audit and explicit canonical JSON fixture synchronization.
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct StalenessArgs {
    #[arg(long, default_value = ".", global = true)]
    base_dir: std::path::PathBuf,
    #[arg(long, global = true)]
    manifest: Option<String>,
    /// Print the authoritative manifest JSON Schema; no files are read or written.
    #[arg(long)]
    schema: bool,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Read-only inventory, canonical ownership, review bindings and fixture comparison.
    Check,
    /// Audit required invariants/negative-test bindings using the same canonical graph.
    Invariants,
    /// Project only declared JSON fixtures. Dry run unless --write is explicitly supplied.
    Sync {
        #[arg(long)]
        write: bool,
    },
}

pub fn run(args: StalenessArgs) -> Result<(), String> {
    if args.schema {
        if args.action.is_some() {
            return Err("--schema cannot be combined with check/sync".into());
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::staleness::schema()).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    match args.action.unwrap_or(Action::Check) {
        action @ (Action::Check | Action::Invariants) => {
            let report = if matches!(action, Action::Invariants) {
                crate::staleness::check_invariants(&args.base_dir, args.manifest.as_deref())?
            } else {
                crate::staleness::check(&args.base_dir, args.manifest.as_deref())?
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            if !report.deterministic_ok {
                return Err(
                    "Test currency/ownership checks require setup or review; see findings above"
                        .into(),
                );
            }
        }
        Action::Sync { write } => {
            let report = crate::staleness::sync(&args.base_dir, args.manifest.as_deref(), write)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            if report.review_required {
                return Err("Canonical inputs or tests need independent review; sync did not update review pins".into());
            }
        }
    }
    Ok(())
}

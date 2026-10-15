//! Explicit policy inspection and hook installation; no model, build or network required.
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct RepositoryArgs {
    #[command(subcommand)]
    pub command: RepositoryCommand,
}

#[derive(Subcommand, Debug)]
pub enum RepositoryCommand {
    /// Audit tracked source, or enforce staged changes and their scope plan.
    Check {
        #[arg(long, default_value = ".")]
        base_dir: PathBuf,
        #[arg(long)]
        staged: bool,
    },
    /// Bind an explicit file list and purpose to the current staged tree.
    Plan {
        #[arg(long, default_value = ".")]
        base_dir: PathBuf,
        #[arg(long)]
        purpose: String,
        #[arg(long, required = true, num_args = 1..)]
        paths: Vec<String>,
    },
    /// Install the deterministic pre-commit check without replacing an existing hook.
    InstallHook {
        #[arg(long, default_value = ".")]
        base_dir: PathBuf,
    },
}

pub fn run(args: RepositoryArgs) -> Result<bool, String> {
    let cfg = crate::config::Config::load();
    if !cfg.capability("repository") || !cfg.tool_enabled("repository_critic") {
        return Err("repository capability is disabled".into());
    }
    match args.command {
        RepositoryCommand::Check { base_dir, staged } => {
            let report = crate::repository::check(&base_dir, staged)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            Ok(report.approval)
        }
        RepositoryCommand::Plan {
            base_dir,
            purpose,
            paths,
        } => {
            crate::repository::load(&base_dir, true)?;
            let result = crate::commit_policy::plan(&base_dir, &purpose, &paths)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
            Ok(true)
        }
        RepositoryCommand::InstallHook { base_dir } => {
            crate::repository::load(&base_dir, false)?;
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let result = crate::commit_policy::install_hook(&base_dir, &exe)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
            Ok(true)
        }
    }
}

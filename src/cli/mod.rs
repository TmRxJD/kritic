//! The `kritic` command line. With no subcommand, kritic runs as an MCP server on stdio (that is
//! how MCP clients start it); in a terminal it prints a short guide instead of waiting silently.

pub mod context;
pub mod detect;
pub mod doctor;
pub mod fetch;
pub mod history;
pub mod register;
pub mod repository;
pub mod setup;
pub mod ui;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "kritic",
    version,
    about = "Experimental evidence-based review for coding agents; no model required",
    long_about = "Test release: deterministic tools produce evidence; optional local or cloud reviewers \
                  help prioritize it. Validate findings independently; approval is scoped to checks.\n\n\
                  First time? Run `kritic setup`.",
    after_help = "With no command, kritic runs as an MCP server on stdio for your coding client."
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Install deterministic tools (models optional), write the config, connect Claude Code / Hermes
    Setup(setup::SetupArgs),
    /// Check every feature and say what to fix
    Doctor(doctor::DoctorArgs),
    /// Connect kritic to Claude Code and/or Hermes
    Register(RegisterArgs),
    /// Remove kritic from Claude Code and Hermes (keeps models and config unless --purge)
    Uninstall(UninstallArgs),
    /// Run the MCP server on stdio (the default when started by an MCP client)
    Serve,
    /// Hermes context-engine backend (used by the kritic Hermes plugin)
    Context(context::ContextArgs),
    /// Save the configured cloud API key (hidden prompt, or piped on stdin)
    Key(KeyArgs),
    /// Audit .kritic/contract.toml without an MCP client or model
    Contract(ContractArgs),
    /// Enforce scoped commits and repository-owned code organization policy
    Repository(repository::RepositoryArgs),
    /// Optional, unverified review history and sanitized jcode-format export; never cached approval.
    History(history::HistoryArgs),
}

#[derive(Args, Debug)]
pub struct ContractArgs {
    #[arg(long, default_value = ".")]
    pub base_dir: std::path::PathBuf,
    #[arg(long)]
    pub path: Option<String>,
    #[arg(long, default_value = "")]
    pub task: String,
    /// Execute the contract's shell commands after you have reviewed them
    #[arg(long)]
    pub run_commands: bool,
}

#[derive(Args, Debug)]
pub struct KeyArgs {
    /// Delete the saved key
    #[arg(long)]
    pub remove: bool,
}

pub fn key(args: KeyArgs) -> Result<(), String> {
    let cfg = crate::config::Config::load();
    let path = &cfg.cloud_key_file;
    if args.remove {
        match std::fs::remove_file(path) {
            Ok(()) => ui::ok(&format!("removed {}", path.display())),
            Err(_) => ui::info(&format!("no key file at {}", path.display())),
        }
        return Ok(());
    }
    let key = if ui::interactive() {
        ui::secret("Configured cloud API key (input hidden)")
    } else {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
        Some(s.trim().to_string()).filter(|s| !s.is_empty())
    }
    .ok_or("no key given")?;
    if key.chars().any(char::is_whitespace) {
        return Err("that doesn't look like a key (it contains whitespace)".into());
    }
    cfg.save_cloud_key(&key)?;
    ui::ok(&format!(
        "key saved to {} (never printed, never in kritic.toml)",
        path.display()
    ));
    match cfg.backend.as_str() {
        "cloud" => ui::info("review.backend = cloud: questions go to your configured provider"),
        "auto" => ui::info(
            "review.backend = auto: local Kev only; cloud requires explicit review.backend = cloud",
        ),
        other => ui::info(&format!(
            "review.backend = {other}: the key is unused until the backend is explicitly cloud"
        )),
    }
    Ok(())
}

#[derive(Args, Debug)]
pub struct RegisterArgs {
    /// Claude Code (default: every agent found)
    #[arg(long)]
    pub claude: bool,
    /// Hermes (default: every agent found)
    #[arg(long)]
    pub hermes: bool,
    /// Also make kritic Hermes' context engine
    #[arg(long)]
    pub hermes_context: bool,
    /// Don't add the end-of-task rule to ~/.claude/CLAUDE.md
    #[arg(long)]
    pub no_rule: bool,
}

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Also delete the kritic folder (models, config, logs, archives)
    #[arg(long)]
    pub purge: bool,
    /// Don't ask before purging
    #[arg(short, long)]
    pub yes: bool,
}

pub fn register(args: RegisterArgs) -> Result<(), String> {
    let home = crate::config::kritic_home();
    let exe = doctor::exe_path();
    let (claude, hermes) = if !args.claude && !args.hermes {
        (register::claude_present(), register::hermes_present())
    } else {
        (args.claude, args.hermes)
    };
    if !claude && !hermes {
        return Err(
            "no agent found: install Claude Code or Hermes, or pass --claude / --hermes".into(),
        );
    }
    if claude {
        register::register_claude(
            &home,
            &exe,
            &register::ClaudeOptions {
                rule: !args.no_rule,
                concise: crate::config::Config::load().output_style == "concise",
            },
        )?;
    }
    if hermes {
        register::register_hermes(
            &home,
            &exe,
            &register::HermesOptions {
                context_engine: args.hermes_context,
                concise: crate::config::Config::load().output_style == "concise",
            },
        )?;
    }
    eprintln!("Start a new agent session to pick kritic up.");
    Ok(())
}

pub fn uninstall(args: UninstallArgs) -> Result<(), String> {
    let home = crate::config::kritic_home();
    let mut done = register::unregister_claude(&home);
    done.extend(register::unregister_hermes(&home));
    for d in &done {
        ui::ok(d);
    }
    if done.is_empty() {
        ui::skip("kritic was not registered anywhere");
    }
    if args.purge {
        if ui::confirm(
            &format!(
                "Delete {} (models, config, logs, archives)?",
                home.display()
            ),
            args.yes,
            args.yes,
        ) {
            std::fs::remove_dir_all(&home).map_err(|e| format!("cannot delete {}: {e} (a running kritic may hold a file; close agent sessions first)", home.display()))?;
            ui::ok(&format!("deleted {}", home.display()));
        } else {
            ui::skip("kept the kritic folder");
        }
    }
    Ok(())
}

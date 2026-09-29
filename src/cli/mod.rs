//! The `kritic` command line. With no subcommand, kritic runs as an MCP server on stdio (that is
//! how MCP clients start it); in a terminal it prints a short guide instead of waiting silently.

pub mod context;
pub mod detect;
pub mod doctor;
pub mod fetch;
pub mod register;
pub mod setup;
pub mod ui;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "kritic",
    version,
    about = "Local, advisory end-of-task review for coding agents",
    long_about = "Local, advisory end-of-task review for coding agents: real tools produce the findings, \
                  the Kev typed-decision model scores them, code decides.\n\n\
                  First time? Run `kritic setup`.",
    after_help = "With no command, kritic runs as an MCP server on stdio (how Claude Code and Hermes start it)."
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Install the model and tools, write the config, connect Claude Code / Hermes
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
    let (claude, hermes) = if !args.claude && !args.hermes { (register::claude_present(), register::hermes_present()) } else { (args.claude, args.hermes) };
    if !claude && !hermes {
        return Err("no agent found: install Claude Code or Hermes, or pass --claude / --hermes".into());
    }
    if claude {
        register::register_claude(&home, &exe, &register::ClaudeOptions { rule: !args.no_rule })?;
    }
    if hermes {
        register::register_hermes(&home, &exe, &register::HermesOptions { context_engine: args.hermes_context })?;
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
        if ui::confirm(&format!("Delete {} (models, config, logs, archives)?", home.display()), args.yes, args.yes) {
            std::fs::remove_dir_all(&home).map_err(|e| format!("cannot delete {}: {e} (a running kritic may hold a file; close agent sessions first)", home.display()))?;
            ui::ok(&format!("deleted {}", home.display()));
        } else {
            ui::skip("kept the kritic folder");
        }
    }
    Ok(())
}

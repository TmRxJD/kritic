use std::io::IsTerminal;

use clap::Parser;
use kritic::cli::{Cli, Cmd};
use kritic::config::Config;
use kritic::server::Kritic;
use rmcp::ServiceExt;

async fn serve() -> anyhow::Result<()> {
    let cfg = Config::load();
    let gov = Kritic::new(cfg.clone());
    tokio::spawn(kritic::events::serve(cfg.ws_port, cfg.log_path.clone()));
    gov.warm();
    let service = gov.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    kritic::events::cleanup(&cfg.log_path);
    Ok(())
}

fn exit_on(result: Result<(), String>) -> ! {
    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Serve) => serve().await,
        // An MCP client pipes stdin; a person at a terminal gets pointed somewhere useful.
        None if std::io::stdin().is_terminal() => {
            eprintln!(
                "kritic {}: local, advisory end-of-task review for coding agents.\n\n  kritic setup     install deterministic tools, connect your agent (models optional)\n  kritic doctor    check what works and what to fix\n  kritic --help    everything else\n\nWith no command and a piped stdin, kritic runs as an MCP server.",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
        None => serve().await,
        Some(Cmd::Setup(a)) => exit_on(kritic::cli::setup::run(a).await),
        Some(Cmd::Doctor(a)) => match kritic::cli::doctor::run(a).await {
            Ok(true) => std::process::exit(0),
            Ok(false) => std::process::exit(1),
            Err(e) => exit_on(Err(e)),
        },
        Some(Cmd::Register(a)) => exit_on(kritic::cli::register(a)),
        Some(Cmd::Uninstall(a)) => exit_on(kritic::cli::uninstall(a)),
        Some(Cmd::Context(a)) => exit_on(kritic::cli::context::run(a).await),
        Some(Cmd::Key(a)) => exit_on(kritic::cli::key(a)),
        Some(Cmd::History(a)) => exit_on(kritic::cli::history::run(a)),
        Some(Cmd::Repository(a)) => match kritic::cli::repository::run(a) {
            Ok(true) => std::process::exit(0),
            Ok(false) => std::process::exit(1),
            Err(e) => exit_on(Err(e)),
        },
        Some(Cmd::Contract(a)) => {
            let cfg = Config::load();
            if !cfg.capability("contracts") || !cfg.tool_enabled("contract_critic") {
                exit_on(Err("contract capability is disabled".into()));
            }
            let result = kritic::contract::critique(
                &cfg,
                &a.base_dir,
                a.path.as_deref(),
                &a.task,
                a.run_commands,
            )
            .await;
            println!("{}", serde_json::to_string_pretty(&result)?);
            std::process::exit(match result.approval {
                Some(true) => 0,
                Some(false) => 1,
                None => 2,
            });
        }
    }
}

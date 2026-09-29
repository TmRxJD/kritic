//! `kritic context <op>`: the Hermes plugin's backend. JSON on stdin, JSON on stdout.

use std::io::Read;
use std::sync::Arc;

use clap::{Args, ValueEnum};
use serde_json::{Value, json};

use crate::config::Config;
use crate::context::{Archive, CompressInput, compress, recall, recall_tool_schema};
use crate::kev::KevClient;

#[derive(Args, Debug)]
pub struct ContextArgs {
    #[arg(value_enum)]
    pub op: ContextOp,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ContextOp {
    /// Archive middle messages until the conversation fits a token target
    Compress,
    /// Read archived messages back by id or search them
    Recall,
    /// Print the kritic_recall tool schema
    Schema,
}

fn read_stdin() -> Result<Value, String> {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).map_err(|e| format!("reading stdin: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("stdin is not JSON: {e}"))
}

pub async fn run(args: ContextArgs) -> Result<(), String> {
    let cfg = Arc::new(Config::load());
    let out = match args.op {
        ContextOp::Schema => recall_tool_schema(),
        ContextOp::Recall => {
            let v = read_stdin()?;
            let archive = Archive::for_session(&cfg.home, v["session_id"].as_str().unwrap_or("default"));
            let max = v["max_chars"].as_u64().unwrap_or(20_000) as usize;
            serde_json::to_value(recall(&archive, v["id"].as_str(), v["query"].as_str(), max)).map_err(|e| e.to_string())?
        }
        ContextOp::Compress => {
            let input: CompressInput = serde_json::from_value(read_stdin()?).map_err(|e| format!("bad compress input: {e}"))?;
            let archive = Archive::for_session(&cfg.home, &input.session_id);
            let kev = KevClient::new(cfg.clone());
            // Rank with Kev only when a backend is usable and the GPU is free soon: a context pass
            // runs inside the agent's turn, so it never waits the full cross-session GPU queue.
            let wants_kev = input.mode == "compress" && input.use_kev;
            let usable = wants_kev && kev.resolve_backend().await.is_ok();
            let mut quick = (*cfg).clone();
            quick.gpu_wait_s = 5;
            let lock = if usable { crate::gpu::lock_across_processes(&quick, "context").await.ok() } else { None };
            let decider = if usable && lock.is_some() { Some(&kev) } else { None };
            let mut r = compress(decider, &archive, &input).await;
            if wants_kev && decider.is_none() {
                r.notes.push(if usable { "GPU busy with another kritic tool; ranked oldest first".into() } else { "no Kev backend; ranked oldest first".into() });
            }
            drop(lock);
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
    };
    println!("{}", serde_json::to_string(&out).unwrap_or_else(|e| json!({"error": e.to_string()}).to_string()));
    Ok(())
}

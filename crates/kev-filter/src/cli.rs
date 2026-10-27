//! Standalone `kev run` and raw-log retrieval; no MCP server or added tool schemas.
use crate::{client, runner};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
const HELP: &str = "kev run [--goal TEXT] [--kev] [--kev-url URL] [--kev-model ID] [--json]\n        [--log-dir DIR] [--budget-chars N] [--timeout SECONDS] [--max-log-bytes N]\n        [--rank-ms N] [--request-ms N] -- PROGRAM [ARG...]\nkev show RUN_DIRECTORY [--stream stdout|stderr] [--from LINE] [--lines N]\n\nNoninteractive only: child stdin is closed. No shell is added; arguments after -- are literal.\nAll raw output is archived before selection. Kev ranking is optional, local-only and advisory.\nDefault limits: 8,000 excerpt characters, 600 seconds, 64 MiB combined raw logs,\n5 seconds total Kev request time / 1 second per call / at most 16 calls.\nExit: child's code normally; 124 timeout, 130 cancellation, 125 wrapper/capture failure,\n127 executor launch failure. See run.json to distinguish child codes from wrapper outcomes.\nLogs default beside the executable in runs/; override with KEV_LOG_DIR or --log-dir.\nNo token savings, diagnostic completeness or model quality is guaranteed.";

pub fn entry() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|s| s == "__kev_worker") {
        std::process::exit(runner::worker(&args[1..]));
    }
    if args.is_empty()
        || args[0] == "--help"
        || args[0] == "-h"
        || (args.len() == 2 && (args[1] == "--help" || args[1] == "-h"))
    {
        println!("{HELP}");
        return;
    }
    if args[0] == "--version" {
        println!("kev-filter {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let result = match args[0].to_str() {
        Some("run") => parse_run(&args[1..]).and_then(runner::run),
        Some("show") => show(&args[1..]),
        _ => Err("unknown command; use --help (use kev --help)".into()),
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("kev: {error}");
            std::process::exit(125);
        }
    }
}

fn number(
    args: &[OsString],
    index: &mut usize,
    name: &str,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    *index += 1;
    let n = args
        .get(*index)
        .and_then(|v| v.to_str())
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| (*v >= min) && (*v <= max))
        .ok_or_else(|| format!("{name} requires a value in {min}..={max}"))?;
    Ok(n)
}
fn value(args: &[OsString], index: &mut usize, name: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .and_then(|v| v.to_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} requires UTF-8 text"))
}

fn parse_run(args: &[OsString]) -> Result<runner::Options, String> {
    let logs = std::env::var_os("KEV_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap_or_else(|_| PathBuf::from("kev"))
                .parent()
                .unwrap_or(Path::new("."))
                .join("runs")
        });
    let mut options = runner::Options {
        command: vec![],
        goal: "Errors, failures, actionable warnings and completion results".into(),
        logs,
        json: false,
        budget: 8000,
        timeout: Duration::from_secs(600),
        max_log_bytes: 64 * 1024 * 1024,
        kev: false,
        kev_url: client::DEFAULT_URL.into(),
        kev_model: String::new(),
        rank_ms: 5000,
        request_ms: 1000,
    };
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_str().ok_or(
            "option names must be UTF-8; command arguments after -- may be arbitrary OS strings",
        )?;
        match flag {
            "--" => {
                options.command = args[index + 1..].to_vec();
                break;
            }
            "--goal" => options.goal = value(args, &mut index, flag)?,
            "--kev" => options.kev = true,
            "--json" => options.json = true,
            "--kev-url" => options.kev_url = value(args, &mut index, flag)?,
            "--kev-model" => options.kev_model = value(args, &mut index, flag)?,
            "--log-dir" => {
                index += 1;
                options.logs = PathBuf::from(args.get(index).ok_or("--log-dir requires a path")?);
            }
            "--budget-chars" => {
                options.budget = number(args, &mut index, flag, 256, 1_000_000)? as usize
            }
            "--timeout" => {
                options.timeout = Duration::from_secs(number(args, &mut index, flag, 1, 86400)?)
            }
            "--max-log-bytes" => {
                options.max_log_bytes = number(args, &mut index, flag, 1024, 1024 * 1024 * 1024)?
            }
            "--rank-ms" => options.rank_ms = number(args, &mut index, flag, 1, 60000)?,
            "--request-ms" => options.request_ms = number(args, &mut index, flag, 1, 10000)?,
            _ => {
                return Err(format!(
                    "unknown option {flag}; separate the command with --"
                ));
            }
        }
        index += 1;
    }
    if options.command.is_empty() {
        return Err("run requires -- PROGRAM [ARG...]".into());
    }
    if options.kev_model.is_empty() {
        options.kev_model = if options.kev_url.ends_with("/systemone") {
            "kev-latest"
        } else {
            "kev-4b"
        }
        .into();
    }
    Ok(options)
}

fn show(args: &[OsString]) -> Result<i32, String> {
    let root = args.first().ok_or("show requires a run directory")?;
    let mut stream = "stdout".to_string();
    let mut from = 1;
    let mut lines = 30;
    let mut index = 1;
    while index < args.len() {
        match args[index].to_str() {
            Some("--stream") => {
                stream = value(args, &mut index, "--stream")?;
                if !matches!(stream.as_str(), "stdout" | "stderr") {
                    return Err("stream must be stdout or stderr".into());
                }
            }
            Some("--from") => from = number(args, &mut index, "--from", 1, u64::MAX)?,
            Some("--lines") => lines = number(args, &mut index, "--lines", 1, 1000)?,
            _ => return Err("unknown show option".into()),
        }
        index += 1;
    }
    use std::io::Read;
    let file = std::fs::File::open(Path::new(root).join(format!("{stream}.log")))
        .map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > 1024 * 1024 * 1024 {
        return Err("show supports raw logs up to 1 GiB".into());
    }
    let mut displayed = 0usize;
    let mut number = 1;
    let mut buffer = Vec::new();
    let mut clipped = false;
    for byte in std::io::BufReader::new(file.take(size))
        .bytes()
        .chain(std::iter::once(Ok(b'\n')))
    {
        let byte = byte.map_err(|e| e.to_string())?;
        if number >= from && buffer.len() < 32000 && byte != b'\n' {
            buffer.push(byte);
        } else if number >= from && byte != b'\n' {
            clipped = true;
        }
        if byte == b'\n' {
            if number >= from {
                let text = runner::filter::sanitize(&String::from_utf8_lossy(&buffer));
                let snippet = text.chars().take(8000 - displayed).collect::<String>();
                displayed += snippet.chars().count();
                println!("[{stream}:{number}] {snippet}");
                if clipped || displayed >= 8000 {
                    eprintln!("kev: retrieval excerpt clipped; raw log unchanged");
                    break;
                }
            }
            number += 1;
            buffer.clear();
            if number >= from.saturating_add(lines) {
                break;
            }
        }
    }
    Ok(0)
}

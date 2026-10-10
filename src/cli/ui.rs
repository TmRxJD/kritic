//! Terminal output and prompts for the CLI. Everything goes to stderr, so `--json` output on
//! stdout stays machine-readable, and prompts fall back to their default when nobody is there to
//! answer (no TTY, or `--yes`).

use std::io::{BufRead, IsTerminal, Write};

static ASKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Questions actually put to the user this run (setup reports it: fewer is better).
pub fn prompts_asked() -> usize {
    ASKED.load(std::sync::atomic::Ordering::Relaxed)
}

fn asked() {
    ASKED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

fn color(code: &str, s: &str) -> String {
    if std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn heading(n: usize, total: usize, title: &str) {
    eprintln!(
        "\n{} {}",
        color("1;36", &format!("[{n}/{total}]")),
        color("1", title)
    );
}

pub fn ok(msg: &str) {
    eprintln!("  {} {msg}", color("32", "ok"));
}

pub fn warn(msg: &str) {
    eprintln!("  {} {msg}", color("33", "!!"));
}

pub fn fail(msg: &str) {
    eprintln!("  {} {msg}", color("31", "xx"));
}

pub fn info(msg: &str) {
    eprintln!("     {msg}");
}

pub fn skip(msg: &str) {
    eprintln!("  {} {msg}", color("2", "--"));
}

/// Ask a yes/no question. `assume` (from `--yes`) or a non-interactive terminal takes `default`.
pub fn confirm(question: &str, default: bool, assume: bool) -> bool {
    if assume || !interactive() {
        return default;
    }
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    asked();
    eprint!("  ? {question} {hint} ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return default;
    }
    match line.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        _ => false,
    }
}

/// Pick one of `options` (value, description). Returns the default without asking when
/// non-interactive.
pub fn choose<'a>(
    question: &str,
    options: &[(&'a str, &str)],
    default: &'a str,
    assume: bool,
) -> &'a str {
    if assume || !interactive() {
        return default;
    }
    asked();
    eprintln!("  ? {question}");
    for (i, (value, what)) in options.iter().enumerate() {
        let mark = if *value == default { " (default)" } else { "" };
        eprintln!("      {}. {value}: {what}{mark}", i + 1);
    }
    eprint!("    choice [1-{}]: ", options.len());
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return default;
    }
    let t = line.trim();
    if let Ok(i) = t.parse::<usize>()
        && (1..=options.len()).contains(&i)
    {
        return options[i - 1].0;
    }
    options
        .iter()
        .find(|(v, _)| *v == t)
        .map(|(v, _)| *v)
        .unwrap_or(default)
}

/// Read a secret without echoing it. None when non-interactive or empty.
pub fn secret(prompt: &str) -> Option<String> {
    if !interactive() {
        return None;
    }
    asked();
    let v = rpassword::prompt_password(format!("  ? {prompt}: ")).ok()?;
    let v = v.trim().to_string();
    (!v.is_empty()).then_some(v)
}

pub fn bytes(n: u64) -> String {
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    const M: f64 = 1024.0 * 1024.0;
    let f = n as f64;
    if f >= G {
        format!("{:.1} GB", f / G)
    } else {
        format!("{:.0} MB", f / M)
    }
}

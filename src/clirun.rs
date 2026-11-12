//! `cli_run`: the CLI builder. A command is declared as data (program, argv array, cwd, stdin text,
//! env) or as a script (language + code) and executed with NO shell in between, so heredocs,
//! `sed`/`awk` one-liners, `python -c "..."` and nested quoting can't mangle it. Windows `.cmd`/`.bat`
//! shims are resolved explicitly; script code is written as UTF-8 (no BOM) to a temp file.
//! `dry_run` returns the resolved command without running it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const DEFAULT_TIMEOUT_S: u64 = 120;
pub const MAX_TIMEOUT_S: u64 = 600;
pub const DEFAULT_MAX_CHARS: usize = 20_000;

#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct Script {
    /// python | node | powershell | bash | cmd
    pub language: String,
    /// The script text, verbatim (any quoting, newlines, heredoc-looking content).
    pub code: String,
    /// Arguments passed to the script (argv after the script path).
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
pub struct CliParams {
    /// Executable name or path, resolved on PATH/PATHEXT (`git`, `cargo`, `npm`). Not a shell: no pipes,
    /// redirects or globs. Mutually exclusive with `script`.
    #[serde(default)]
    pub program: Option<String>,
    /// Arguments, one array element each, passed verbatim (no quoting needed).
    #[serde(default)]
    pub args: Vec<String>,
    /// Run a script instead of a program. Use this for multi-line code instead of heredocs or `-c`.
    #[serde(default)]
    pub script: Option<Script>,
    /// Working directory (default: the server's).
    #[serde(default)]
    pub cwd: Option<String>,
    /// Text written to the process's stdin, verbatim. Replaces heredocs and `echo ... |`.
    #[serde(default)]
    pub stdin: Option<String>,
    /// Extra environment variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Timeout in seconds (default 120, max 600). The process tree is killed on timeout.
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// Max characters kept per stream (head and tail are kept). Default 20000.
    #[serde(default)]
    pub max_output_chars: Option<usize>,
    /// Resolve and validate only; return the command line without running it.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CliResult {
    pub tool: String,
    /// false when the command was rejected, could not start, or timed out. A nonzero exit code is
    /// reported in `exit_code`, not as an error.
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// Fully resolved program path.
    pub resolved_program: Option<String>,
    /// The argv actually used (script path included).
    pub argv: Vec<String>,
    /// Human-readable command line, quoted for the platform. Display only; it is never re-parsed.
    pub display: Option<String>,
    pub cwd: Option<String>,
    pub duration_ms: u64,
    pub dry_run: bool,
    pub error: Option<String>,
    /// Always null: cli_run reports an outcome, it does not approve anything.
    pub approval: Option<bool>,
}

impl CliResult {
    fn failed(error: impl Into<String>) -> Self {
        CliResult {
            tool: "cli_run".into(),
            ok: false,
            exit_code: None,
            timed_out: false,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            resolved_program: None,
            argv: vec![],
            display: None,
            cwd: None,
            duration_ms: 0,
            dry_run: false,
            error: Some(error.into()),
            approval: None,
        }
    }
}

/// Shells whose inline-command flags re-introduce the quoting layer this tool exists to remove.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "cmd", "powershell", "pwsh"];
const INLINE_FLAGS: &[&str] = &[
    "-c",
    "/c",
    "/k",
    "-command",
    "-encodedcommand",
    "-e",
    "-ec",
    "-lc",
];

/// Reject shapes that are a shell string in disguise. Returns a message telling the caller what to use.
pub fn validate(p: &CliParams) -> Result<(), String> {
    match (&p.program, &p.script) {
        (Some(_), Some(_)) => return Err("give either `program` or `script`, not both".into()),
        (None, None) => return Err("give `program` (+ `args`) or `script` {language, code}".into()),
        _ => {}
    }
    if let Some(prog) = &p.program {
        if prog.trim().is_empty() {
            return Err("`program` is empty".into());
        }
        if prog.contains(char::is_whitespace) && !Path::new(prog).exists() {
            return Err(format!(
                "`program` must be just an executable, not a command line: {prog:?}. Put each argument in `args`."
            ));
        }
        let stem = Path::new(prog)
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if SHELLS.contains(&stem.as_str()) {
            if let Some(a) = p
                .args
                .iter()
                .find(|a| INLINE_FLAGS.contains(&a.to_ascii_lowercase().as_str()))
            {
                return Err(format!(
                    "`{stem} {a} ...` is a shell string in disguise. Use `script` {{language, code}} so the code is passed as a file."
                ));
            }
        }
    }
    if let Some(s) = &p.script {
        if interpreter(&s.language).is_none() {
            return Err(format!(
                "unknown script language {:?}; use python, node, powershell, bash or cmd",
                s.language
            ));
        }
        if s.code.trim().is_empty() {
            return Err("`script.code` is empty".into());
        }
    }
    if let Some(t) = p.timeout_s
        && t == 0
    {
        return Err("`timeout_s` must be at least 1".into());
    }
    Ok(())
}

/// (program, leading args, file extension) for a script language.
fn interpreter(lang: &str) -> Option<(&'static str, &'static [&'static str], &'static str)> {
    Some(match lang.to_ascii_lowercase().as_str() {
        "python" | "py" => ("python", &["-I", "-X", "utf8"], "py"),
        "node" | "js" => ("node", &[], "js"),
        "powershell" | "pwsh" | "ps1" => {
            ("pwsh", &["-NoProfile", "-NonInteractive", "-File"], "ps1")
        }
        "bash" | "sh" => ("bash", &[], "sh"),
        "cmd" | "bat" => ("cmd", &["/d", "/c"], "cmd"),
        _ => return None,
    })
}

/// Resolve `program` against PATH and PATHEXT (Windows) without a shell.
pub fn resolve(program: &str) -> Option<PathBuf> {
    let p = Path::new(program);
    let has_dir = p.components().count() > 1;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or(".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .map(|s| s.to_ascii_lowercase())
            .collect()
    } else {
        vec![]
    };
    let try_in = |dir: &Path| -> Option<PathBuf> {
        let base = dir.join(program);
        if base.is_file() && (!cfg!(windows) || base.extension().is_some()) {
            return Some(base);
        }
        for e in &exts {
            let cand = PathBuf::from(format!("{}{}", base.display(), e));
            if cand.is_file() {
                return Some(cand);
            }
        }
        None
    };
    if has_dir {
        return try_in(Path::new("."));
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .find_map(|d| try_in(&d))
}

/// Quote one argument for display in a Windows/PowerShell-agnostic way (display only).
pub fn quote_display(a: &str) -> String {
    if !a.is_empty() && !a.contains(|c: char| c.is_whitespace() || "\"'&|<>()^;$`".contains(c)) {
        a.to_string()
    } else {
        format!("\"{}\"", a.replace('"', "\\\""))
    }
}

/// Keep head and tail of `s` within `max` characters.
pub fn clip(s: &str, max: usize) -> (String, bool) {
    let n = s.chars().count();
    if n <= max {
        return (s.to_string(), false);
    }
    let head = max * 2 / 3;
    let tail = max - head;
    let h: String = s.chars().take(head).collect();
    let t: String = s.chars().skip(n - tail).collect();
    (format!("{h}\n…[{} chars elided]…\n{t}", n - max), true)
}

pub async fn run(p: CliParams) -> CliResult {
    let t0 = Instant::now();
    if let Err(e) = validate(&p) {
        return CliResult::failed(e);
    }
    let max = p
        .max_output_chars
        .unwrap_or(DEFAULT_MAX_CHARS)
        .clamp(200, 400_000);
    let timeout = Duration::from_secs(p.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S).min(MAX_TIMEOUT_S));
    let cwd = p.cwd.clone().map(PathBuf::from);
    if let Some(c) = &cwd
        && !c.is_dir()
    {
        return CliResult::failed(format!("cwd is not a directory: {}", c.display()));
    }

    // Build program + argv (script: write the code to a temp file first).
    let _tmp; // keeps the script file alive until the process exits
    let (prog_name, argv): (String, Vec<String>) = if let Some(s) = &p.script {
        let (prog, lead, ext) = interpreter(&s.language).expect("validated");
        let dir = match tempfile::Builder::new().prefix("kritic-cli-").tempdir() {
            Ok(d) => d,
            Err(e) => return CliResult::failed(format!("temp dir: {e}")),
        };
        let file = dir.path().join(format!("script.{ext}"));
        // UTF-8, no BOM; pwsh -File reads BOM-less UTF-8 correctly. CRLF is kept as given.
        if let Err(e) = std::fs::write(&file, s.code.as_bytes()) {
            return CliResult::failed(format!("write script: {e}"));
        }
        let mut a: Vec<String> = lead.iter().map(|x| x.to_string()).collect();
        a.push(file.display().to_string());
        a.extend(s.args.iter().cloned());
        _tmp = Some(dir);
        (prog.to_string(), a)
    } else {
        _tmp = None;
        (p.program.clone().unwrap(), p.args.clone())
    };
    let Some(resolved) = resolve(&prog_name) else {
        return CliResult::failed(format!("program not found on PATH: {prog_name}"));
    };
    let display = std::iter::once(quote_display(&resolved.display().to_string()))
        .chain(argv.iter().map(|a| quote_display(a)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut res = CliResult::failed("");
    res.error = None;
    res.resolved_program = Some(resolved.display().to_string());
    res.argv = argv.clone();
    res.display = Some(display);
    res.cwd = cwd.as_ref().map(|c| c.display().to_string());
    res.dry_run = p.dry_run;
    if p.dry_run {
        res.ok = true;
        return res;
    }

    let mut cmd = tokio::process::Command::new(&resolved);
    cmd.args(&argv)
        .stdin(if p.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(c) = &cwd {
        cmd.current_dir(c);
    }
    for (k, v) in &p.env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            res.error = Some(format!("failed to start {}: {e}", resolved.display()));
            res.duration_ms = t0.elapsed().as_millis() as u64;
            return res;
        }
    };
    if let (Some(text), Some(mut si)) = (p.stdin.clone(), child.stdin.take()) {
        tokio::spawn(async move {
            let _ = si.write_all(text.as_bytes()).await;
            let _ = si.shutdown().await;
        });
    }
    let mut so = child.stdout.take().expect("piped");
    let mut se = child.stderr.take().expect("piped");
    let out_task = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b).await;
        b
    });
    let err_task = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b).await;
        b
    });
    let waited = tokio::time::timeout(timeout, child.wait()).await;
    match waited {
        Ok(Ok(status)) => res.exit_code = status.code(),
        Ok(Err(e)) => res.error = Some(format!("wait failed: {e}")),
        Err(_) => {
            res.timed_out = true;
            kill_tree(&mut child).await;
            res.error = Some(format!(
                "timed out after {}s; process tree killed",
                timeout.as_secs()
            ));
        }
    }
    let grab = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
    let so_s = grab(
        tokio::time::timeout(Duration::from_secs(2), out_task)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default(),
    );
    let se_s = grab(
        tokio::time::timeout(Duration::from_secs(2), err_task)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default(),
    );
    (res.stdout, res.stdout_truncated) = clip(&so_s, max);
    (res.stderr, res.stderr_truncated) = clip(&se_s, max);
    res.ok = res.error.is_none();
    res.duration_ms = t0.elapsed().as_millis() as u64;
    res
}

async fn kill_tree(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(p: &str, a: &[&str]) -> CliParams {
        CliParams {
            program: Some(p.into()),
            args: a.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn rejects_shell_strings_and_ambiguous_shapes() {
        assert!(validate(&prog("bash", &["-c", "echo hi"])).is_err());
        assert!(validate(&prog("pwsh", &["-Command", "ls"])).is_err());
        assert!(validate(&prog("cmd.exe", &["/c", "dir"])).is_err());
        assert!(validate(&prog("git status", &[])).is_err());
        assert!(validate(&CliParams::default()).is_err());
        let both = CliParams {
            script: Some(Script {
                language: "python".into(),
                code: "1".into(),
                args: vec![],
            }),
            ..prog("git", &[])
        };
        assert!(validate(&both).is_err());
        assert!(validate(&prog("git", &["status"])).is_ok());
        let bad_lang = CliParams {
            script: Some(Script {
                language: "perl".into(),
                code: "1".into(),
                args: vec![],
            }),
            ..Default::default()
        };
        assert!(validate(&bad_lang).is_err());
    }

    #[test]
    fn clip_keeps_head_and_tail() {
        let s = "a".repeat(100) + &"b".repeat(100);
        let (c, t) = clip(&s, 60);
        assert!(t && c.starts_with("aaaa") && c.ends_with("bbbb") && c.contains("elided"));
        assert_eq!(clip("short", 60), ("short".to_string(), false));
    }

    #[test]
    fn quoting_is_display_only() {
        assert_eq!(quote_display("plain"), "plain");
        assert_eq!(quote_display("a b"), "\"a b\"");
        assert_eq!(quote_display(""), "\"\"");
    }

    #[tokio::test]
    async fn dry_run_resolves_without_running() {
        let r = run(CliParams {
            dry_run: true,
            ..prog("git", &["--version"])
        })
        .await;
        assert!(
            r.ok && r.dry_run && r.exit_code.is_none() && r.display.unwrap().contains("--version")
        );
    }

    #[tokio::test]
    async fn unknown_program_is_an_error_not_a_panic() {
        let r = run(prog("definitely-not-a-real-program-xyz", &[])).await;
        assert!(!r.ok && r.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn awkward_args_and_stdin_arrive_verbatim() {
        // quotes, $, backticks, %VAR%, newlines, unicode: none may be interpreted
        let nasty = "it's \"q\" $HOME `x` %PATH% \n line2 é✓";
        let code = "import sys\nsys.stdout.buffer.write(sys.argv[1].encode('utf8') + b'|' + sys.stdin.buffer.read())";
        let r = run(CliParams {
            script: Some(Script {
                language: "python".into(),
                code: code.into(),
                args: vec![nasty.into()],
            }),
            stdin: Some("<<EOF\nnot a heredoc\nEOF".into()),
            ..Default::default()
        })
        .await;
        assert!(r.ok, "{:?}", r.error);
        assert_eq!(r.exit_code, Some(0));
        assert_eq!(r.stdout, format!("{nasty}|<<EOF\nnot a heredoc\nEOF"));
    }

    #[tokio::test]
    async fn nonzero_exit_is_reported_not_an_error() {
        let r = run(CliParams {
            script: Some(Script {
                language: "python".into(),
                code: "import sys;sys.stderr.write('boom');sys.exit(3)".into(),
                args: vec![],
            }),
            ..Default::default()
        })
        .await;
        assert!(r.ok && r.exit_code == Some(3) && r.stderr == "boom");
    }

    #[tokio::test]
    async fn timeout_kills_and_reports() {
        let r = run(CliParams {
            script: Some(Script {
                language: "python".into(),
                code: "import time;time.sleep(30)".into(),
                args: vec![],
            }),
            timeout_s: Some(1),
            ..Default::default()
        })
        .await;
        assert!(!r.ok && r.timed_out && r.duration_ms < 15_000);
    }

    #[tokio::test]
    async fn cwd_and_env_apply() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = BTreeMap::new();
        env.insert("KRITIC_CLI_T".to_string(), "v 1".to_string());
        let r = run(CliParams {
            script: Some(Script {
                language: "python".into(),
                code: "import os;print(os.getcwd());print(os.environ['KRITIC_CLI_T'])".into(),
                args: vec![],
            }),
            cwd: Some(dir.path().display().to_string()),
            env,
            ..Default::default()
        })
        .await;
        assert!(r.ok);
        let mut lines = r.stdout.lines();
        let got = std::fs::canonicalize(lines.next().unwrap()).unwrap();
        assert_eq!(got, std::fs::canonicalize(dir.path()).unwrap());
        assert_eq!(lines.next(), Some("v 1"));
    }

    #[tokio::test]
    async fn bad_cwd_is_rejected() {
        let r = run(CliParams {
            cwd: Some("Z:/no/such/dir".into()),
            ..prog("git", &["--version"])
        })
        .await;
        assert!(!r.ok && r.error.unwrap().contains("cwd"));
    }
}

//! Connect kritic to the agents that use it, and undo it.
//!
//! - **Claude Code**: the MCP entry (through `claude mcp add` when the CLI is installed, else
//!   `~/.claude.json`), the `kritic` skill, the `ux-critic` agent, and a marked block in
//!   `~/.claude/CLAUDE.md` that tells Claude when to run the review.
//! - **Hermes**: the MCP entry in `$HERMES_HOME/config.yaml`, the kritic context-engine plugin in
//!   `$HERMES_HOME/plugins/kritic/`, and optionally `context.engine: kritic`.
//!
//! Every file changed is backed up to `<kritic home>/backups/` first, and YAML is edited line by
//! line so the user's comments survive.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use super::ui;

pub const SKILL_MD: &str = include_str!("../../examples/claude/skills/kritic/SKILL.md");
pub const SETUP_REFERENCE: &str =
    include_str!("../../examples/claude/skills/kritic/references/setup.md");
pub const AGENT_MD: &str = include_str!("../../examples/claude/agents/ux-critic.md");
pub const CLAUDE_RULE: &str = include_str!("../../examples/claude/CLAUDE.md");
pub const CONCISE_RULE: &str = include_str!("../../examples/claude/CONCISE.md");
pub const CONCISE_STYLE: &str =
    include_str!("../../examples/claude/output-styles/kritic-concise.md");
const STYLE_NAME: &str = "kritic-concise";
pub const HERMES_PLUGIN_INIT: &str = include_str!("../../integrations/hermes/kritic/__init__.py");
pub const HERMES_PLUGIN_YAML: &str = include_str!("../../integrations/hermes/kritic/plugin.yaml");

const BEGIN: &str =
    "<!-- kritic:begin (managed by `kritic register`; `kritic uninstall` removes it) -->";
const END: &str = "<!-- kritic:end -->";

fn user_home() -> PathBuf {
    crate::config::env_var("USERPROFILE")
        .or_else(|| crate::config::env_var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn claude_dir() -> PathBuf {
    crate::config::env_var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home().join(".claude"))
}

pub fn claude_json() -> PathBuf {
    user_home().join(".claude.json")
}

pub fn hermes_home() -> PathBuf {
    // Hermes keeps its data in %LOCALAPPDATA%\hermes on native Windows, ~/.hermes elsewhere.
    crate::config::env_var("HERMES_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| match crate::config::env_var("LOCALAPPDATA") {
            Some(l) if cfg!(windows) => PathBuf::from(l).join("hermes"),
            _ => user_home().join(".hermes"),
        })
}

pub fn claude_present() -> bool {
    crate::config::which("claude").is_some() || claude_dir().is_dir() || claude_json().is_file()
}

pub fn hermes_present() -> bool {
    crate::config::which("hermes").is_some() || hermes_home().join("config.yaml").is_file()
}

/// Copy `path` into `<home>/backups/` with a timestamp before changing it.
fn backup(home: &Path, path: &Path) {
    if !path.is_file() {
        return;
    }
    let dir = home.join("backups");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _ = std::fs::copy(path, dir.join(format!("{name}.{stamp}")));
}

/// Forward slashes: valid on every platform and free of JSON/YAML escaping.
pub fn portable(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// Environment the MCP entry must carry: only a non-default kritic home.
fn entry_env(home: &Path) -> Vec<(String, String)> {
    let default = user_home().join(".kritic");
    if crate::files::dunce_canonical(home) == crate::files::dunce_canonical(&default) {
        vec![]
    } else {
        vec![("KRITIC_HOME".into(), portable(home))]
    }
}

// ------------------------------------------------------------------ Claude Code

pub struct ClaudeOptions {
    pub rule: bool,
    /// Install and select the concise output style (`output.style = "concise"`).
    pub concise: bool,
}

pub fn register_claude(home: &Path, exe: &Path, opts: &ClaudeOptions) -> Result<(), String> {
    let env = entry_env(home);
    if let Some(claude) = crate::config::which("claude") {
        let _ = Command::new(&claude)
            .args(["mcp", "remove", "kritic", "-s", "user"])
            .output();
        let mut cmd = Command::new(&claude);
        cmd.args(["mcp", "add", "--scope", "user", "kritic"]);
        for (k, v) in &env {
            cmd.args(["-e", &format!("{k}={v}")]);
        }
        cmd.arg("--").arg(exe);
        let out = cmd.output().map_err(|e| format!("claude mcp add: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "claude mcp add failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        ui::ok("MCP server registered with `claude mcp add` (user scope)");
    } else {
        let path = claude_json();
        backup(home, &path);
        let mut v: Value = if path.is_file() {
            serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("{}: {e}", path.display()))?
        } else {
            json!({})
        };
        let env_obj: serde_json::Map<String, Value> = env
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let servers = v
            .as_object_mut()
            .ok_or("~/.claude.json is not an object")?
            .entry("mcpServers")
            .or_insert_with(|| json!({}));
        servers
            .as_object_mut()
            .ok_or("mcpServers is not an object")?
            .insert(
                "kritic".into(),
                json!({"type": "stdio", "command": portable(exe), "args": [], "env": env_obj}),
            );
        write_atomic(
            &path,
            &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"),
        )?;
        ui::ok(&format!("MCP server added to {}", path.display()));
    }
    let skill = claude_dir().join("skills").join("kritic").join("SKILL.md");
    write_file(&skill, SKILL_MD)?;
    write_file(
        &claude_dir().join("skills/kritic/references/setup.md"),
        SETUP_REFERENCE,
    )?;
    ui::ok(&format!("skill installed: {}", skill.display()));
    let agent = claude_dir().join("agents").join("ux-critic.md");
    if agent.is_file() && std::fs::read_to_string(&agent).is_ok_and(|t| !t.contains("kritic")) {
        backup(home, &agent);
        ui::warn(&format!(
            "replaced an existing ux-critic agent (backup in {})",
            home.join("backups").display()
        ));
    }
    write_file(&agent, AGENT_MD)?;
    ui::ok(&format!("agent installed: {}", agent.display()));
    if opts.concise {
        install_concise_style(home)?;
    }
    if opts.rule || opts.concise {
        let md = claude_dir().join("CLAUDE.md");
        backup(home, &md);
        let text = std::fs::read_to_string(&md).unwrap_or_default();
        let body = match (opts.rule, opts.concise) {
            (true, true) => format!("{}\n\n{}", CLAUDE_RULE.trim(), CONCISE_RULE.trim()),
            (true, false) => CLAUDE_RULE.trim().to_string(),
            _ => CONCISE_RULE.trim().to_string(),
        };
        write_file(&md, &upsert_block(&text, &body))?;
        ui::ok(&format!(
            "{} added to {}",
            if opts.rule {
                "end-of-task rule"
            } else {
                "concise-reply rule"
            },
            md.display()
        ));
    }
    Ok(())
}

/// The concise output style: installed as a file, and selected in settings.json unless the user
/// already chose another style (their choice wins; the CLAUDE.md rule still applies).
fn install_concise_style(home: &Path) -> Result<(), String> {
    let style = claude_dir()
        .join("output-styles")
        .join(format!("{STYLE_NAME}.md"));
    write_file(&style, CONCISE_STYLE)?;
    let settings = claude_dir().join("settings.json");
    let mut v: Value = match std::fs::read_to_string(&settings) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", settings.display()))?,
        Err(_) => json!({}),
    };
    let obj = v.as_object_mut().ok_or("settings.json is not an object")?;
    match obj.get("outputStyle").and_then(Value::as_str) {
        Some(s) if s == STYLE_NAME => ui::ok(&format!("output style: {STYLE_NAME}")),
        Some(s) if !s.eq_ignore_ascii_case("default") => {
            ui::info(&format!(
                "output style \"{s}\" kept (yours); the concise rule is in CLAUDE.md, or pick {STYLE_NAME} with /output-style"
            ));
        }
        _ => {
            backup(home, &settings);
            obj.insert("outputStyle".into(), json!(STYLE_NAME));
            write_atomic(
                &settings,
                &(serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n"),
            )?;
            ui::ok(&format!(
                "output style: {STYLE_NAME} (set in {})",
                settings.display()
            ));
        }
    }
    Ok(())
}

pub fn unregister_claude(home: &Path) -> Vec<String> {
    let mut done = Vec::new();
    if let Some(claude) = crate::config::which("claude")
        && Command::new(&claude)
            .args(["mcp", "remove", "kritic", "-s", "user"])
            .output()
            .is_ok_and(|o| o.status.success())
    {
        done.push("removed the MCP entry (claude mcp remove)".into());
    }
    let path = claude_json();
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(mut v) = serde_json::from_str::<Value>(&text)
        && v.pointer_mut("/mcpServers")
            .and_then(Value::as_object_mut)
            .is_some_and(|m| m.shift_remove("kritic").is_some())
    {
        backup(home, &path);
        if serde_json::to_string_pretty(&v).is_ok_and(|t| write_atomic(&path, &(t + "\n")).is_ok())
        {
            done.push(format!("removed the MCP entry from {}", path.display()));
        }
    }
    let skill = claude_dir().join("skills").join("kritic");
    if skill.is_dir() && std::fs::remove_dir_all(&skill).is_ok() {
        done.push(format!("removed {}", skill.display()));
    }
    let agent = claude_dir().join("agents").join("ux-critic.md");
    if std::fs::read_to_string(&agent).is_ok_and(|t| t.contains("kritic"))
        && std::fs::remove_file(&agent).is_ok()
    {
        done.push(format!("removed {}", agent.display()));
    }
    let md = claude_dir().join("CLAUDE.md");
    if let Ok(text) = std::fs::read_to_string(&md)
        && text.contains(BEGIN)
    {
        backup(home, &md);
        if write_file(&md, &remove_block(&text)).is_ok() {
            done.push(format!("removed the kritic block from {}", md.display()));
        }
    }
    let style = claude_dir()
        .join("output-styles")
        .join(format!("{STYLE_NAME}.md"));
    if style.is_file() && std::fs::remove_file(&style).is_ok() {
        done.push(format!("removed {}", style.display()));
    }
    let settings = claude_dir().join("settings.json");
    if let Ok(text) = std::fs::read_to_string(&settings)
        && let Ok(mut v) = serde_json::from_str::<Value>(&text)
        && v.get("outputStyle").and_then(Value::as_str) == Some(STYLE_NAME)
        && let Some(obj) = v.as_object_mut()
    {
        backup(home, &settings);
        obj.shift_remove("outputStyle");
        if serde_json::to_string_pretty(&v)
            .is_ok_and(|t| write_atomic(&settings, &(t + "\n")).is_ok())
        {
            done.push(format!(
                "output style reset to default in {}",
                settings.display()
            ));
        }
    }
    done
}

/// Insert or replace the managed block, leaving everything else untouched.
pub fn upsert_block(text: &str, body: &str) -> String {
    let block = format!("{BEGIN}\n{body}\n{END}");
    if let (Some(a), Some(b)) = (text.find(BEGIN), text.find(END))
        && b > a
    {
        return format!("{}{block}{}", &text[..a], &text[b + END.len()..]);
    }
    let sep = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{text}{sep}{block}\n")
}

pub fn remove_block(text: &str) -> String {
    match (text.find(BEGIN), text.find(END)) {
        (Some(a), Some(b)) if b > a => {
            let rest = text[b + END.len()..]
                .strip_prefix('\n')
                .unwrap_or(&text[b + END.len()..]);
            format!(
                "{}{rest}",
                text[..a].trim_end_matches('\n').to_string() + if a > 0 { "\n" } else { "" }
            )
        }
        _ => text.to_string(),
    }
}

// ------------------------------------------------------------------ Hermes

pub struct HermesOptions {
    /// Make kritic the active context engine (`context.engine: kritic`).
    pub context_engine: bool,
    /// Add the concise-reply rule to every turn (`output.style = "concise"`).
    pub concise: bool,
}

pub fn register_hermes(home: &Path, exe: &Path, opts: &HermesOptions) -> Result<(), String> {
    let hh = hermes_home();
    let cfg = hh.join("config.yaml");
    std::fs::create_dir_all(&hh).map_err(|e| e.to_string())?;
    backup(home, &cfg);
    let mut text = std::fs::read_to_string(&cfg).unwrap_or_default();
    let mut entry = vec![
        format!("command: \"{}\"", portable(exe)),
        "args: []".to_string(),
    ];
    let env = entry_env(home);
    if !env.is_empty() {
        entry.push("env:".into());
        entry.extend(env.iter().map(|(k, v)| format!("  {k}: \"{v}\"")));
    }
    text = yaml_upsert_child(&text, "mcp_servers", "kritic", &entry)?;
    if opts.context_engine {
        text = yaml_upsert_scalar(&text, "context", "engine", "kritic")?;
    }
    write_atomic(&cfg, &text)?;
    ui::ok(&format!("MCP server added to {}", cfg.display()));
    let plugin = hh.join("plugins").join("kritic");
    write_file(&plugin.join("__init__.py"), HERMES_PLUGIN_INIT)?;
    write_file(&plugin.join("plugin.yaml"), HERMES_PLUGIN_YAML)?;
    // The plugin runs `kritic context ...`; tell it where this binary is.
    write_file(&plugin.join("kritic_exe.txt"), &portable(exe))?;
    let concise = plugin.join("concise.md");
    if opts.concise {
        write_file(&concise, CONCISE_RULE)?;
        ui::ok("concise-reply rule added to every Hermes turn (output.style = \"concise\")");
    } else {
        let _ = std::fs::remove_file(&concise);
    }
    ui::ok(&format!(
        "context engine plugin installed: {}",
        plugin.display()
    ));
    // Hermes leaves user plugins disabled until asked; do the asking when the CLI is there.
    if let Some(h) = crate::config::which("hermes") {
        match Command::new(h)
            .args(["plugins", "enable", "kritic"])
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(o) if o.status.success() => ui::ok("plugin enabled (hermes plugins enable kritic)"),
            _ => ui::warn("could not enable the plugin: run `hermes plugins enable kritic`"),
        }
    } else {
        ui::info("Hermes CLI not on PATH: run `hermes plugins enable kritic` once");
    }
    if opts.context_engine {
        ui::ok("Hermes now uses kritic as its context engine (context.engine: kritic)");
    } else {
        ui::info("To let kritic manage Hermes' context: kritic register --hermes --hermes-context");
    }
    Ok(())
}

pub fn unregister_hermes(home: &Path) -> Vec<String> {
    let mut done = Vec::new();
    let hh = hermes_home();
    let cfg = hh.join("config.yaml");
    if let Ok(text) = std::fs::read_to_string(&cfg) {
        let mut t = yaml_remove_child(&text, "mcp_servers", "kritic");
        if yaml_get_scalar(&t, "context", "engine").as_deref() == Some("kritic") {
            t = yaml_upsert_scalar(&t, "context", "engine", "compressor").unwrap_or(t);
            done.push("context.engine set back to compressor".into());
        }
        if t != text {
            backup(home, &cfg);
            if write_atomic(&cfg, &t).is_ok() {
                done.push(format!("removed kritic from {}", cfg.display()));
            }
        }
    }
    let plugin = hh.join("plugins").join("kritic");
    if plugin.is_dir() && std::fs::remove_dir_all(&plugin).is_ok() {
        done.push(format!("removed {}", plugin.display()));
    }
    done
}

// ------------------------------------------------------------------ line-based YAML edits

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn is_content(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && !t.starts_with('#')
}

/// The line range of top-level `section:` and its body (until the next top-level key).
fn section_range(lines: &[&str], section: &str) -> Option<(usize, usize)> {
    let head = lines.iter().position(|l| {
        indent_of(l) == 0
            && l.trim_end().split('#').next().unwrap_or("").trim_end() == format!("{section}:")
    })?;
    let end = (head + 1..lines.len())
        .find(|&i| is_content(lines[i]) && indent_of(lines[i]) == 0)
        .unwrap_or(lines.len());
    Some((head, end))
}

/// Indentation of the section's children (2 when the section is empty).
fn child_indent(lines: &[&str], head: usize, end: usize) -> usize {
    (head + 1..end)
        .map(|i| lines[i])
        .find(|l| is_content(l))
        .map(indent_of)
        .filter(|&n| n > 0)
        .unwrap_or(2)
}

/// Lines of child `key` inside the section: its header and every deeper line after it.
fn child_range(
    lines: &[&str],
    head: usize,
    end: usize,
    ind: usize,
    key: &str,
) -> Option<(usize, usize)> {
    let start = (head + 1..end).find(|&i| {
        indent_of(lines[i]) == ind && lines[i].trim_start().starts_with(&format!("{key}:"))
    })?;
    let stop = (start + 1..end)
        .find(|&i| is_content(lines[i]) && indent_of(lines[i]) <= ind)
        .unwrap_or(end);
    Some((start, stop))
}

fn join(lines: &[String], trailing_newline: bool) -> String {
    let mut s = lines.join("\n");
    if trailing_newline {
        s.push('\n');
    }
    s
}

/// Set `section.key` to a mapping given as `body` lines (relative indentation), replacing an
/// existing `key` block. An inline section value (`section: {}`) is refused rather than guessed at.
pub fn yaml_upsert_child(
    text: &str,
    section: &str,
    key: &str,
    body: &[String],
) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some((head, end)) = section_range(&lines, section) else {
        if lines
            .iter()
            .any(|l| indent_of(l) == 0 && l.starts_with(&format!("{section}:")))
        {
            return Err(format!(
                "`{section}:` is written inline in config.yaml; add kritic under it by hand"
            ));
        }
        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(format!("{section}:"));
        out.push(format!("  {key}:"));
        out.extend(body.iter().map(|b| format!("    {b}")));
        return Ok(join(&out, true));
    };
    let ind = child_indent(&lines, head, end);
    let block: Vec<String> = std::iter::once(format!("{}{key}:", " ".repeat(ind)))
        .chain(body.iter().map(|b| format!("{}{b}", " ".repeat(ind * 2))))
        .collect();
    let mut out: Vec<String> = Vec::new();
    match child_range(&lines, head, end, ind, key) {
        Some((s, e)) => {
            out.extend(lines[..s].iter().map(|l| l.to_string()));
            out.extend(block);
            out.extend(lines[e..].iter().map(|l| l.to_string()));
        }
        None => {
            out.extend(lines[..=head].iter().map(|l| l.to_string()));
            out.extend(block);
            out.extend(lines[head + 1..].iter().map(|l| l.to_string()));
        }
    }
    Ok(join(&out, text.ends_with('\n') || text.is_empty()))
}

pub fn yaml_remove_child(text: &str, section: &str, key: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some((head, end)) = section_range(&lines, section) else {
        return text.to_string();
    };
    let ind = child_indent(&lines, head, end);
    let Some((s, e)) = child_range(&lines, head, end, ind, key) else {
        return text.to_string();
    };
    let out: Vec<String> = lines[..s]
        .iter()
        .chain(&lines[e..])
        .map(|l| l.to_string())
        .collect();
    join(&out, text.ends_with('\n'))
}

/// Set a scalar `section.key: value`.
pub fn yaml_upsert_scalar(
    text: &str,
    section: &str,
    key: &str,
    value: &str,
) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some((head, end)) = section_range(&lines, section) else {
        if lines
            .iter()
            .any(|l| indent_of(l) == 0 && l.starts_with(&format!("{section}:")))
        {
            return Err(format!(
                "`{section}:` is written inline in config.yaml; set {section}.{key} by hand"
            ));
        }
        let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(format!("{section}:"));
        out.push(format!("  {key}: {value}"));
        return Ok(join(&out, true));
    };
    let ind = child_indent(&lines, head, end);
    let line = format!("{}{key}: {value}", " ".repeat(ind));
    let mut out: Vec<String> = Vec::new();
    match child_range(&lines, head, end, ind, key) {
        Some((s, e)) => {
            out.extend(lines[..s].iter().map(|l| l.to_string()));
            out.push(line);
            out.extend(lines[e..].iter().map(|l| l.to_string()));
        }
        None => {
            out.extend(lines[..=head].iter().map(|l| l.to_string()));
            out.push(line);
            out.extend(lines[head + 1..].iter().map(|l| l.to_string()));
        }
    }
    Ok(join(&out, text.ends_with('\n') || text.is_empty()))
}

pub fn yaml_get_scalar(text: &str, section: &str, key: &str) -> Option<String> {
    let v: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    v.get(section)?.get(key)?.as_str().map(String::from)
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Write via a temp file and rename, so a crash never leaves a half-written config.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("kritic-tmp");
    write_file(&tmp, text)?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

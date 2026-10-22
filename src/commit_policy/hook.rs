//! Local hooks are opt-in, bypassable safeguards. Existing hooks are never overwritten.
use super::git;
use schemars::JsonSchema;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, JsonSchema)]
pub struct HookReport {
    pub path: String,
    pub installed: bool,
    pub local_bypassable: bool,
    pub note: String,
}

fn executable_setting(value: &str) -> Result<String, String> {
    if value.contains(['\0', '\r', '\n']) {
        return Err("Hook paths cannot contain NUL or newline".into());
    }
    #[cfg(windows)]
    let value = value
        .strip_prefix("\\\\?\\")
        .unwrap_or(value)
        .replace('\\', "/");
    Ok(value.to_string())
}

const HOOK: &str = "#!/bin/sh\n# kritic managed pre-commit v2; local and bypassable\nset -eu\nROOT=$(git rev-parse --show-toplevel)\nKRITIC_EXECUTABLE=${KRITIC_BIN:-}\nif [ -z \"$KRITIC_EXECUTABLE\" ]; then\n  KRITIC_EXECUTABLE=$(git config --get kritic.executable || :)\nfi\nif [ -z \"$KRITIC_EXECUTABLE\" ]; then\n  KRITIC_EXECUTABLE=kritic\nfi\nexec \"$KRITIC_EXECUTABLE\" repository check --staged --base-dir \"$ROOT\"\n";

pub fn install_hook(base: &Path, exe: &Path) -> Result<HookReport, String> {
    let root_raw = git::output(base, &["rev-parse", "--show-toplevel"])?;
    let root = String::from_utf8(root_raw).map_err(|e| e.to_string())?;
    let root = root.trim_end_matches(['\r', '\n']);
    // --git-path honors core.hooksPath and linked-worktree private directories.
    let raw = git::output(
        Path::new(root),
        &["rev-parse", "--git-path", "hooks/pre-commit"],
    )?;
    let hook_raw = String::from_utf8(raw).map_err(|e| e.to_string())?;
    let hook = PathBuf::from(hook_raw.trim_end_matches(['\r', '\n']));
    let hook = if hook.is_absolute() {
        hook
    } else {
        Path::new(root).join(hook)
    };
    let executable = exe
        .canonicalize()
        .map_err(|e| format!("Resolve Kritic executable: {e}"))?;
    if !executable.is_file() {
        return Err("Kritic executable must be a regular file".into());
    }
    let setting = executable_setting(
        executable
            .to_str()
            .ok_or("Kritic executable path must be UTF-8")?,
    )?;
    if hook.exists() {
        let current =
            std::fs::read_to_string(&hook).map_err(|e| format!("Read existing hook: {e}"))?;
        if current != HOOK {
            return Err(format!(
                "Existing pre-commit hook preserved at {}; add a reviewed Kritic invocation to it manually",
                hook.display()
            ));
        }
        git::output(base, &["config", "--local", "kritic.executable", &setting])?;
        return Ok(HookReport { path: hook.display().to_string(), installed: false, local_bypassable: true, note: "Identical Kritic hook already installed; required CI is needed for server enforcement".into() });
    }
    let parent = hook.parent().ok_or("Hook has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    // create_new prevents racing an independently created hook.
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&hook)
        .map_err(|e| format!("Create hook without overwriting: {e}"))?;
    file.write_all(HOOK.as_bytes()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    git::output(base, &["config", "--local", "kritic.executable", &setting])?;
    Ok(HookReport { path: hook.display().to_string(), installed: true, local_bypassable: true, note: "Required CI must run the same policy for server enforcement; inspect the staged diff against its declared purpose".into() })
}

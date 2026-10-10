//! Process and resource hygiene: what a project, or one test run, leaves behind. Facts only.
//!
//! - [`audit`] (`hygiene_critic`): the machine as it is now. Processes whose executable or
//!   command line sits under a project root, which of them are orphans (parent gone), listening
//!   sockets still bound after their owner exited (a child inherited the handle), and leftover
//!   files matching globs. `clean` kills orphans and socket inheritors; files are only reported.
//! - [`snapshot`] before a test launches the app, then [`leaks`] after it closed: processes and
//!   listening ports the run created that are still alive.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sysinfo::System;

use super::procs::{self, belongs, cmdline, norm, pid, start_time};

pub const MAX_ITEMS: usize = 50;

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A parent counts only if it is the process that started this one (PIDs get reused).
fn parent_alive(sys: &System, p: &sysinfo::Process) -> bool {
    p.parent()
        .and_then(|pp| sys.process(pp))
        .is_some_and(|parent| start_time(parent) <= start_time(p))
}

fn describe(p: &sysinfo::Process, ports: &HashMap<u32, Vec<u16>>) -> Value {
    let cmd: String = cmdline(p).chars().take(200).collect();
    json!({
        "pid": p.pid().as_u32(),
        "ppid": p.parent().map(|x| x.as_u32()),
        "name": p.name().to_string_lossy(),
        "cmd": cmd,
        "age_s": now_s().saturating_sub(start_time(p)),
        "ports": ports.get(&p.pid().as_u32()).cloned().unwrap_or_default(),
    })
}

pub fn norm_roots(roots: &[String]) -> Vec<String> {
    roots
        .iter()
        .filter(|r| !r.trim().is_empty())
        .map(|r| norm(Path::new(r)))
        .collect()
}

fn project_pids(sys: &System, roots: &[String], exclude: &HashSet<u32>) -> Vec<u32> {
    let me = std::process::id();
    let mut v: Vec<u32> = sys
        .processes()
        .iter()
        .filter(|(p, proc_)| {
            let n = p.as_u32();
            n != me && !exclude.contains(&n) && belongs(proc_, roots)
        })
        .map(|(p, _)| p.as_u32())
        .collect();
    v.sort_unstable();
    v
}

/// Listening sockets whose owner has exited, with the owner's children still alive (the likely
/// holders of the inherited handle).
fn dead_owner_sockets(sys: &System) -> Vec<Value> {
    procs::listeners()
        .into_iter()
        .filter(|(_, owner)| *owner != 0 && *owner != 4 && sys.process(pid(*owner)).is_none())
        .map(|(port, owner)| {
            let heirs: Vec<Value> = sys
                .processes()
                .values()
                .filter(|p| p.parent().is_some_and(|pp| pp.as_u32() == owner))
                .map(|p| json!({"pid": p.pid().as_u32(), "name": p.name().to_string_lossy()}))
                .collect();
            json!({"port": port, "owner_pid": owner, "inheritors": heirs})
        })
        .collect()
}

fn expand(pattern: &str) -> String {
    let home = crate::config::env_var("USERPROFILE")
        .or_else(|| crate::config::env_var("HOME"))
        .unwrap_or_default();
    let p = if let Some(rest) = pattern.strip_prefix('~') {
        format!("{home}{rest}")
    } else {
        pattern.to_string()
    };
    // %VAR% (Windows) and $VAR / ${VAR}.
    let re = regex::Regex::new(r"%([A-Za-z0-9_]+)%|\$\{([A-Za-z0-9_]+)\}|\$([A-Za-z0-9_]+)")
        .expect("regex");
    re.replace_all(&p, |c: &regex::Captures| {
        let name = c
            .get(1)
            .or(c.get(2))
            .or(c.get(3))
            .map(|m| m.as_str())
            .unwrap_or("");
        std::env::var(name).unwrap_or_default()
    })
    .into_owned()
}

/// Files matching a glob with `*`, `?` and `**`.
pub fn glob(pattern: &str) -> Vec<PathBuf> {
    let pattern = expand(pattern).replace('\\', "/");
    let parts: Vec<&str> = pattern.split('/').collect();
    // The longest literal prefix is the directory to start walking from.
    let lit = parts
        .iter()
        .take_while(|p| !p.contains(['*', '?', '[']))
        .count();
    if lit == parts.len() {
        let p = PathBuf::from(&pattern);
        return if p.exists() { vec![p] } else { vec![] };
    }
    let base = if lit == 0 {
        PathBuf::from(".")
    } else {
        PathBuf::from(
            parts[..lit].join("/")
                + if lit == 1 && parts[0].ends_with(':') {
                    "/"
                } else {
                    ""
                },
        )
    };
    let rest: Vec<String> = parts[lit..].iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    walk(&base, &rest, &mut out, 0);
    out.sort();
    out
}

fn seg_match(pat: &str, name: &str) -> bool {
    let re = format!(
        "^{}$",
        regex::escape(pat).replace(r"\*", ".*").replace(r"\?", ".")
    );
    regex::RegexBuilder::new(&re)
        .case_insensitive(cfg!(windows))
        .build()
        .is_ok_and(|r| r.is_match(name))
}

fn walk(dir: &Path, rest: &[String], out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 32 || out.len() > 10_000 {
        return;
    }
    let Some((head, tail)) = rest.split_first() else {
        out.push(dir.to_path_buf());
        return;
    };
    if head == "**" {
        walk(dir, tail, out, depth + 1);
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                walk(&e.path(), rest, out, depth + 1);
            }
        }
        return;
    }
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if seg_match(head, &name) {
            if tail.is_empty() {
                out.push(e.path());
            } else if e.path().is_dir() {
                walk(&e.path(), tail, out, depth + 1);
            }
        }
    }
}

pub fn audit(
    roots: &[String],
    ignore_names: &[String],
    leftover_globs: &[String],
    clean: bool,
) -> Value {
    let roots = norm_roots(roots);
    if roots.is_empty() {
        return json!({"ok": false, "error": "hygiene: no roots given"});
    }
    let ignore: HashSet<String> = ignore_names.iter().map(|n| n.to_lowercase()).collect();
    let sys = procs::system();
    let ports = procs::ports_by_pid();
    let mut listed = Vec::new();
    let mut orphans: HashSet<u32> = HashSet::new();
    for p in project_pids(&sys, &roots, &HashSet::new()) {
        let Some(proc_) = sys.process(pid(p)) else {
            continue;
        };
        if ignore.contains(&proc_.name().to_string_lossy().to_lowercase()) {
            continue;
        }
        let mut d = describe(proc_, &ports);
        let orphan = !parent_alive(&sys, proc_);
        d["orphan"] = json!(orphan);
        if orphan {
            orphans.insert(p);
        }
        listed.push(d);
    }
    let sockets = dead_owner_sockets(&sys);
    let mut killed = Vec::new();
    if clean {
        let mut targets = orphans.clone();
        for s in &sockets {
            for h in s["inheritors"].as_array().into_iter().flatten() {
                if let Some(p) = h["pid"].as_u64() {
                    targets.insert(p as u32);
                }
            }
        }
        killed = procs::kill_all(&sys, &targets);
    }
    let mut leftovers: Vec<String> = leftover_globs
        .iter()
        .flat_map(|g| glob(g))
        .map(|p| p.display().to_string())
        .collect();
    leftovers.sort();
    leftovers.dedup();
    let orphan_list: Vec<Value> = listed
        .iter()
        .filter(|d| d["orphan"] == true)
        .take(MAX_ITEMS)
        .cloned()
        .collect();
    json!({
        "ok": true,
        "roots": roots,
        "processes": listed.into_iter().take(MAX_ITEMS).collect::<Vec<_>>(),
        "orphans": orphan_list,
        "dead_owner_sockets": sockets.into_iter().take(MAX_ITEMS).collect::<Vec<_>>(),
        "leftovers": leftovers.into_iter().take(MAX_ITEMS).collect::<Vec<_>>(),
        "killed": killed,
        "cleaned": clean,
    })
}

/// What existed before a run: project processes and listening sockets.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub pids: HashSet<u32>,
    pub listen: HashSet<(u16, u32)>,
    pub t: u64,
    pub roots: Vec<String>,
}

pub fn snapshot(roots: &[String]) -> Snapshot {
    let roots = norm_roots(roots);
    let sys = procs::system();
    Snapshot {
        pids: project_pids(&sys, &roots, &HashSet::new())
            .into_iter()
            .collect(),
        listen: procs::listeners().into_iter().collect(),
        t: now_s(),
        roots,
    }
}

/// After the run: project processes it started that are still alive, and listening ports it
/// left bound. Killed when `kill`, always reported.
pub fn leaks(before: &Snapshot, kill: bool) -> Value {
    let sys = procs::system();
    let ports = procs::ports_by_pid();
    let leaked: Vec<u32> = project_pids(&sys, &before.roots, &before.pids)
        .into_iter()
        .filter(|p| {
            sys.process(pid(*p))
                .is_some_and(|x| start_time(x) + 1 >= before.t)
        })
        .collect();
    let described: Vec<Value> = leaked
        .iter()
        .filter_map(|p| sys.process(pid(*p)).map(|x| describe(x, &ports)))
        .collect();
    let mut new_ports = Vec::new();
    for (port, owner) in procs::listeners() {
        if before.listen.contains(&(port, owner)) {
            continue;
        }
        let alive = sys.process(pid(owner));
        // Only ports the run owns: held by a leaked process, or by an owner that exited.
        if leaked.contains(&owner) || alive.is_none() {
            let name = alive
                .map(|p| p.name().to_string_lossy().into_owned())
                .unwrap_or_else(|| "exited owner (inherited handle)".into());
            new_ports.push(json!({"port": port, "pid": owner, "name": name}));
        }
    }
    if kill {
        procs::kill_all(&sys, &leaked.iter().copied().collect());
    }
    json!({"processes": described.into_iter().take(MAX_ITEMS).collect::<Vec<_>>(), "ports": new_ports.into_iter().take(MAX_ITEMS).collect::<Vec<_>>()})
}

/// What a run owns: the app's folder, the command's program folder, its cwd, extra roots.
pub fn roots_for(
    exe: Option<&str>,
    command: Option<&Value>,
    cwd: Option<&str>,
    extra: &[String],
) -> Vec<String> {
    let mut roots = Vec::new();
    if let Some(e) = exe.and_then(|e| Path::new(e).parent()) {
        roots.push(e.display().to_string());
    }
    if let Some(p) = command
        .and_then(procs::command_program)
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        roots.push(p.display().to_string());
    }
    if let Some(c) = cwd.filter(|c| !c.is_empty()) {
        roots.push(c.to_string());
    }
    roots.extend(extra.iter().cloned());
    roots
}

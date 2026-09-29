//! Process table, process trees, listening sockets and per-process counters, natively.
//!
//! `sysinfo` gives the cross-platform basics; Windows-only counters (handles, private bytes,
//! threads) come from Win32, Linux ones from `/proc`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// A full process table with exe paths and command lines.
pub fn system() -> System {
    let mut sys = System::new();
    refresh(&mut sys);
    sys
}

pub fn refresh(sys: &mut System) {
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_disk_usage()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet),
    );
}

pub fn pid(n: u32) -> Pid {
    Pid::from_u32(n)
}

/// `root` and every descendant.
pub fn tree_pids(sys: &System, root: u32) -> HashSet<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (p, proc_) in sys.processes() {
        if let Some(parent) = proc_.parent() {
            children
                .entry(parent.as_u32())
                .or_default()
                .push(p.as_u32());
        }
    }
    let mut out = HashSet::from([root]);
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        for &c in children.get(&p).into_iter().flatten() {
            if out.insert(c) {
                stack.push(c);
            }
        }
    }
    out
}

/// (port, pid) of every listening TCP socket.
pub fn listeners() -> Vec<(u16, u32)> {
    use netstat2::{
        AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState, get_sockets_info,
    };
    let Ok(socks) = get_sockets_info(
        AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6,
        ProtocolFlags::TCP,
    ) else {
        return vec![];
    };
    let mut out = Vec::new();
    for s in socks {
        if let ProtocolSocketInfo::Tcp(t) = &s.protocol_socket_info
            && t.state == TcpState::Listen
        {
            for pid in &s.associated_pids {
                out.push((t.local_port, *pid));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn ports_by_pid() -> HashMap<u32, Vec<u16>> {
    let mut m: HashMap<u32, Vec<u16>> = HashMap::new();
    for (port, pid) in listeners() {
        m.entry(pid).or_default().push(port);
    }
    m
}

/// Case-insensitive on Windows, trailing separators removed.
pub fn norm(p: &Path) -> String {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    let s = abs
        .display()
        .to_string()
        .replace('/', std::path::MAIN_SEPARATOR_STR);
    let s = s.trim_end_matches(['\\', '/']).to_string();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

pub fn under(path: &Path, roots: &[String]) -> bool {
    let p = norm(path);
    let sep = std::path::MAIN_SEPARATOR;
    roots
        .iter()
        .any(|r| p == *r || p.starts_with(&format!("{r}{sep}")))
}

fn looks_absolute(arg: &str) -> bool {
    Path::new(arg).is_absolute()
        || (arg.len() > 3 && arg.as_bytes()[1] == b':' && matches!(arg.as_bytes()[2], b'\\' | b'/'))
}

/// A process belongs to the project when its executable, or any absolute path in its command
/// line, sits under one of `roots` (normalized with [`norm`]).
pub fn belongs(proc_: &sysinfo::Process, roots: &[String]) -> bool {
    if proc_.exe().is_some_and(|e| under(e, roots)) {
        return true;
    }
    proc_
        .cmd()
        .iter()
        .map(|a| a.to_string_lossy().trim_matches('"').to_string())
        .any(|a| looks_absolute(&a) && under(Path::new(&a), roots))
}

/// Seconds since the epoch the process started.
pub fn start_time(proc_: &sysinfo::Process) -> u64 {
    proc_.start_time()
}

pub fn cmdline(proc_: &sysinfo::Process) -> String {
    proc_
        .cmd()
        .iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Per-process counters `sysinfo` doesn't provide on every platform.
#[derive(Debug, Clone, Copy, Default)]
pub struct Extra {
    pub handles: u64,
    pub private_bytes: u64,
    pub threads: u64,
}

/// Thread counts of every process, from one snapshot (Windows: Toolhelp).
#[cfg(windows)]
pub fn thread_counts() -> HashMap<u32, u64> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let mut out = HashMap::new();
    // SAFETY: plain Win32 calls with a correctly sized struct; the handle is closed below.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                out.insert(e.th32ProcessID, e.cntThreads as u64);
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
pub fn thread_counts() -> HashMap<u32, u64> {
    HashMap::new()
}

#[cfg(windows)]
pub fn extra(pid: u32, threads: &HashMap<u32, u64>) -> Extra {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{
        GetProcessHandleCount, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let mut x = Extra {
        threads: threads.get(&pid).copied().unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: the handle comes from OpenProcess and is closed before returning; the counters
    // struct is sized for the EX variant the call is told about.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return x;
        };
        let mut n = 0u32;
        if GetProcessHandleCount(h, &mut n).is_ok() {
            x.handles = n as u64;
        }
        let mut mc = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        if GetProcessMemoryInfo(h, &mut mc as *mut _ as *mut PROCESS_MEMORY_COUNTERS, mc.cb).is_ok()
        {
            x.private_bytes = mc.PrivateUsage as u64;
        }
        let _ = CloseHandle(h);
    }
    x
}

#[cfg(target_os = "linux")]
pub fn extra(pid: u32, _threads: &HashMap<u32, u64>) -> Extra {
    let fds = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .map(|d| d.count() as u64)
        .unwrap_or(0);
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    let field = |k: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(k))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
    };
    Extra {
        handles: fds,
        private_bytes: field("RssAnon:") * 1024,
        threads: field("Threads:"),
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn extra(_pid: u32, _threads: &HashMap<u32, u64>) -> Extra {
    Extra::default()
}

/// Kill every process in `pids` (children first) and wait briefly for them to go.
pub fn kill_all(sys: &System, pids: &HashSet<u32>) -> Vec<String> {
    let mut killed = Vec::new();
    let mut order: Vec<u32> = pids.iter().copied().collect();
    // Deeper processes (started later) first, so a parent doesn't respawn a child.
    order.sort_by_key(|p| std::cmp::Reverse(sys.process(pid(*p)).map(start_time).unwrap_or(0)));
    for p in order {
        if let Some(proc_) = sys.process(pid(p))
            && proc_.kill()
        {
            killed.push(format!("{p} {}", proc_.name().to_string_lossy()));
        }
    }
    killed
}

/// The first absolute path in a command (string or argv), for deciding what a run owns.
pub fn command_program(command: &serde_json::Value) -> Option<PathBuf> {
    let first = match command {
        serde_json::Value::Array(a) => a.first()?.as_str()?.to_string(),
        serde_json::Value::String(s) => {
            let s = s.trim();
            if let Some(rest) = s.strip_prefix('"') {
                rest.split('"').next()?.to_string()
            } else {
                s.split_whitespace().next()?.to_string()
            }
        }
        _ => return None,
    };
    let p = PathBuf::from(first);
    p.is_absolute().then_some(p)
}

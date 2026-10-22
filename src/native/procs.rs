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

/// A process lifetime, not merely a reusable process number. Windows uses FILETIME;
/// Linux uses the kernel's start-time ticks. Never compare identities across platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Identity {
    pub pid: u32,
    pub born: u64,
}

fn valid_child(parent: Identity, child: Identity) -> bool {
    parent.pid != child.pid && parent.born > 0 && child.born >= parent.born
}

fn same_identity(expected: Identity, observed: Identity) -> bool {
    expected == observed && expected.born > 0
}

/// Handles pin the process lifetime used for both identity checks and termination.
struct VerifiedProcess {
    identity: Identity,
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
    #[cfg(target_os = "linux")]
    handle: std::os::fd::OwnedFd,
}

#[cfg(windows)]
impl VerifiedProcess {
    fn open(pid: u32, owned: bool) -> Result<Self, String> {
        use windows::Win32::Foundation::{CloseHandle, FILETIME};
        use windows::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        };
        let mut access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
        if owned {
            access |= PROCESS_TERMINATE;
        }
        // SAFETY: OpenProcess returns an owned handle; every failure and Drop closes it.
        unsafe {
            let handle = OpenProcess(access, false, pid).map_err(|e| e.to_string())?;
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            if let Err(e) =
                GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
            {
                let _ = CloseHandle(handle);
                return Err(e.to_string());
            }
            let born =
                (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
            if born == 0 {
                let _ = CloseHandle(handle);
                return Err("process creation time unavailable".into());
            }
            Ok(Self {
                identity: Identity { pid, born },
                handle,
            })
        }
    }

    fn alive(&self) -> bool {
        use windows::Win32::Foundation::WAIT_TIMEOUT;
        use windows::Win32::System::Threading::WaitForSingleObject;
        // SAFETY: this instance owns a valid synchronizable process handle.
        unsafe { WaitForSingleObject(self.handle, 0) == WAIT_TIMEOUT }
    }

    fn terminate(&self) -> Result<(), String> {
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
        // SAFETY: creation identity and termination refer to the very same owned handle.
        unsafe {
            TerminateProcess(self.handle, 1).map_err(|e| e.to_string())?;
            if WaitForSingleObject(self.handle, 2000) != WAIT_OBJECT_0 {
                return Err("process termination did not complete within 2 seconds".into());
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for VerifiedProcess {
    fn drop(&mut self) {
        // SAFETY: this instance uniquely owns the handle.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_birth(pid: u32) -> Result<u64, String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    // comm can itself contain spaces and parentheses; fields after its final ')' are stable.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(19))
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .ok_or_else(|| "process creation time unavailable".into())
}

#[cfg(target_os = "linux")]
impl VerifiedProcess {
    fn open(pid: u32, _owned: bool) -> Result<Self, String> {
        use std::os::fd::FromRawFd;
        let born = linux_birth(pid)?;
        // SAFETY: pidfd_open takes an integer PID and zero flags, and returns an owned fd.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: the successful syscall transferred one new fd to this instance.
        let handle = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) };
        let p = Self {
            identity: Identity { pid, born },
            handle,
        };
        if linux_birth(pid)? != born || !p.alive() {
            return Err("process exited or PID was reused while opening pidfd".into());
        }
        Ok(p)
    }

    fn alive(&self) -> bool {
        use std::os::fd::AsRawFd;
        let mut p = libc::pollfd {
            fd: self.handle.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: p is one initialized pollfd and the descriptor remains owned here.
        unsafe { libc::poll(&mut p, 1, 0) == 0 }
    }

    fn terminate(&self) -> Result<(), String> {
        use std::os::fd::AsRawFd;
        // SAFETY: this pidfd pins the verified process; null siginfo and flags 0 are valid.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.handle.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0u32,
            )
        };
        if result < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut p = libc::pollfd {
            fd: self.handle.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: p is a valid pollfd for the retained process descriptor.
        if unsafe { libc::poll(&mut p, 1, 2000) } <= 0 {
            return Err("process termination did not complete within 2 seconds".into());
        }
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
impl VerifiedProcess {
    fn open(_pid: u32, _owned: bool) -> Result<Self, String> {
        Err("verified process sampling is supported on Windows and Linux".into())
    }
    fn alive(&self) -> bool {
        false
    }
    fn terminate(&self) -> Result<(), String> {
        Err("verified termination unavailable".into())
    }
}

/// Retain observed ancestry even after a parent exits. Old identities are never
/// replaced with a new process that happens to reuse a tracked PID.
pub(crate) struct TrackedTree {
    root: Identity,
    owned: bool,
    known: HashMap<u32, VerifiedProcess>,
}

impl TrackedTree {
    pub fn new(root: u32, owned: bool) -> Result<Self, String> {
        let process = VerifiedProcess::open(root, owned)?;
        let identity = process.identity;
        Ok(Self {
            root: identity,
            owned,
            known: HashMap::from([(root, process)]),
        })
    }

    pub fn root_alive(&self) -> bool {
        self.known[&self.root.pid].alive()
    }

    pub fn refresh(&mut self, sys: &System) -> HashSet<Identity> {
        loop {
            let mut discovered = Vec::new();
            for (pid, process) in sys.processes() {
                let p = pid.as_u32();
                if self.known.contains_key(&p) {
                    continue;
                }
                let Some(parent) = process.parent().and_then(|p| self.known.get(&p.as_u32()))
                else {
                    continue;
                };
                // Linux reparents children at exit. An old numeric PPID is never
                // sufficient to add new descendants to a dead tracked parent.
                #[cfg(target_os = "linux")]
                if !parent.alive() || !identity_still_current(parent.identity) {
                    continue;
                }
                if let Ok(child) = VerifiedProcess::open(p, self.owned)
                    && valid_child(parent.identity, child.identity)
                    // Re-read ancestry after opening the handle: the process table
                    // row itself could precede PID reuse. Windows handles pin PIDs.
                    && current_parent(p) == Some(parent.identity.pid)
                    && child.alive()
                {
                    discovered.push((p, child));
                }
            }
            if discovered.is_empty() {
                break;
            }
            self.known.extend(discovered);
        }
        self.known
            .values()
            .filter(|p| {
                p.alive() && sys.process(pid(p.identity.pid)).is_some()
            // A pidfd pins Linux identity but does not prevent reuse of the numeric PID.
            && identity_still_current(p.identity)
            })
            .map(|p| p.identity)
            .collect()
    }

    pub fn cleanup(&self) -> Vec<String> {
        if !self.owned {
            return Vec::new();
        }
        let mut processes: Vec<_> = self.known.values().collect();
        processes.sort_by_key(|p| std::cmp::Reverse(p.identity.born));
        processes
            .into_iter()
            .filter(|p| p.alive())
            .filter_map(|p| {
                p.terminate()
                    .err()
                    .map(|e| format!("PID {}: {e}", p.identity.pid))
            })
            .collect()
    }
}

#[cfg(target_os = "linux")]
fn identity_still_current(identity: Identity) -> bool {
    linux_birth(identity.pid).is_ok_and(|born| {
        same_identity(
            identity,
            Identity {
                pid: identity.pid,
                born,
            },
        )
    })
}
#[cfg(not(target_os = "linux"))]
fn identity_still_current(identity: Identity) -> bool {
    same_identity(identity, identity)
}

#[cfg(windows)]
fn current_parent(pid: u32) -> Option<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: a fresh process snapshot is closed on every path after creation.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = None;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                if entry.th32ProcessID == pid {
                    found = Some(entry.th32ParentProcessID);
                    break;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

#[cfg(target_os = "linux")]
fn current_parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn current_parent(_pid: u32) -> Option<u32> {
    None
}

#[cfg(test)]
mod identity_tests {
    use super::{Identity, same_identity, valid_child};

    #[test]
    fn stale_parent_pid_does_not_adopt_an_older_process() {
        let reused_parent = Identity {
            pid: 20,
            born: 1_000_009,
        };
        let unrelated = Identity {
            pid: 30,
            born: 1_000_001,
        };
        assert!(!valid_child(reused_parent, unrelated));
        assert!(valid_child(
            reused_parent,
            Identity {
                pid: 40,
                born: 1_000_010
            }
        ));
        // Precision matters: all timestamps above can fall within one wall-clock second.
    }

    #[test]
    fn unknown_identity_and_self_parent_are_rejected() {
        assert!(!valid_child(
            Identity { pid: 1, born: 0 },
            Identity { pid: 2, born: 1 }
        ));
        assert!(!valid_child(
            Identity { pid: 1, born: 1 },
            Identity { pid: 2, born: 0 }
        ));
        assert!(!valid_child(
            Identity { pid: 1, born: 1 },
            Identity { pid: 1, born: 2 }
        ));
    }

    #[test]
    fn reused_pid_is_not_the_recorded_lifetime() {
        let recorded = Identity { pid: 42, born: 100 };
        assert!(same_identity(recorded, recorded));
        assert!(!same_identity(recorded, Identity { pid: 42, born: 101 }));
        assert!(!same_identity(recorded, Identity { pid: 43, born: 100 }));
    }
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod lifecycle_tests {
    use super::{TrackedTree, refresh, system};
    use std::net::TcpListener;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const PARENT: &str = "native::procs::lifecycle_tests::parent_fixture";
    const CHILD: &str = "native::procs::lifecycle_tests::child_fixture";

    // Fixtures only execute when explicitly selected by a subprocess. Ordinary
    // full-suite runs do not spawn an accidental second fixture tree.
    fn selected(name: &str) -> bool {
        std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|args| args[0] == "--exact" && args[1] == name)
    }

    fn fixture(name: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", name, "--nocapture"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    struct RootGuard(Child);
    impl Drop for RootGuard {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) && self.0.kill().is_ok() {
                let _ = self.0.wait();
            }
        }
    }

    #[test]
    fn child_fixture() {
        if !selected(CHILD) {
            return;
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write("child-ready.tmp", format!("{} {port}", std::process::id())).unwrap();
        std::fs::rename("child-ready.tmp", "child-ready").unwrap();
        // A bounded fallback prevents a leaked fixture if its controlling test panics.
        std::thread::sleep(Duration::from_secs(60));
        drop(listener);
        panic!("fixture was not cleaned up before its safety deadline");
    }

    #[test]
    fn parent_fixture() {
        if !selected(PARENT) {
            return;
        }
        let mut child = RootGuard(fixture(CHILD).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(30);
        while !std::path::Path::new("release-parent").exists() {
            assert!(
                Instant::now() < deadline,
                "parent fixture readiness deadline exceeded"
            );
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "child fixture exited before release"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // Deliberately orphan the verified child. Its safety deadline still bounds
        // lifetime if the controlling test fails before it can perform cleanup.
        std::mem::forget(child);
    }

    #[test]
    fn observed_child_remains_owned_after_parent_exit_and_is_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let mut root = RootGuard(fixture(PARENT).current_dir(dir.path()).spawn().unwrap());
        let mut tracked = TrackedTree::new(root.0.id(), true).unwrap();
        let mut sys = system();
        let deadline = Instant::now() + Duration::from_secs(20);
        let (child_pid, port) = loop {
            refresh(&mut sys);
            let alive = tracked.refresh(&sys);
            if let Ok(ready) = std::fs::read_to_string(dir.path().join("child-ready")) {
                let fields: Vec<_> = ready.split_whitespace().collect();
                assert_eq!(fields.len(), 2, "malformed fixture readiness: {ready}");
                let pid: u32 = fields[0].parse().unwrap();
                let port: u16 = fields[1].parse().unwrap();
                if alive.iter().any(|identity| identity.pid == pid) {
                    assert_eq!(
                        alive.len(),
                        2,
                        "root and verified child must both be live: {alive:?}"
                    );
                    assert!(alive.iter().any(|identity| identity.pid == root.0.id()));
                    break (pid, port);
                }
            }
            assert!(
                root.0.try_wait().unwrap().is_none(),
                "parent fixture exited before ownership was verified"
            );
            assert!(
                Instant::now() < deadline,
                "child readiness and verified ownership deadline exceeded"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            TcpListener::bind(("127.0.0.1", port)).is_err(),
            "fixture must hold the port before cleanup"
        );
        std::fs::write(dir.path().join("release-parent"), "verified").unwrap();
        let exit_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = root.0.try_wait().unwrap() {
                assert!(status.success(), "parent fixture failed: {status}");
                break;
            }
            assert!(
                Instant::now() < exit_deadline,
                "parent did not exit after release"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        refresh(&mut sys);
        let alive = tracked.refresh(&sys);
        assert!(!tracked.root_alive());
        assert_eq!(
            alive.len(),
            1,
            "only the verified orphan may remain: {alive:?}"
        );
        assert_eq!(alive.iter().next().unwrap().pid, child_pid);
        assert!(
            tracked.cleanup().is_empty(),
            "verified descendant cleanup failed"
        );
        refresh(&mut sys);
        assert!(
            tracked.refresh(&sys).is_empty(),
            "a tracked process survived cleanup"
        );
        assert!(
            TcpListener::bind(("127.0.0.1", port)).is_ok(),
            "owned descendant still holds port {port}"
        );
    }
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

//! Run ownership is observed ancestry plus process creation identity, never paths or age.
//! Keep identities after parents exit so an already observed orphan remains owned.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Identity {
    pub pid: u32,
    pub born: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Process {
    pub identity: Identity,
    pub parent: Option<u32>,
}

pub(super) struct Ownership {
    root: Identity,
    known: HashSet<Identity>,
}

impl Ownership {
    pub fn new(root: Identity) -> Self {
        Self {
            root,
            known: HashSet::from([root]),
        }
    }

    /// Only adopt a child while its exact parent is present and authenticated. An absent or
    /// reused parent PID cannot establish new ancestry. Unknown creation times fail closed.
    pub fn observe(&mut self, table: &[Process]) -> HashSet<Identity> {
        let live: HashMap<u32, Identity> =
            table.iter().map(|p| (p.identity.pid, p.identity)).collect();
        loop {
            let mut added = false;
            for p in table {
                if self.known.contains(&p.identity) {
                    continue;
                }
                if let Some(parent) = p.parent.and_then(|p| live.get(&p))
                    && self.known.contains(parent)
                    && parent.born <= p.identity.born
                {
                    added |= self.known.insert(p.identity);
                }
            }
            if !added {
                break;
            }
        }
        live.values()
            .filter(|p| self.known.contains(p))
            .copied()
            .collect()
    }

    pub fn root(&self) -> Identity {
        self.root
    }
}

/// High-resolution creation identity; sysinfo's integer seconds can alias a reused PID.
#[cfg(windows)]
fn process(pid: u32) -> Option<Process> {
    use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_INFORMATION,
    };
    // SAFETY: creation time and ancestry come from the same kernel handle, so a stale
    // sysinfo snapshot cannot combine a reused PID's identity with its predecessor's parent.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION, false, pid).ok()?;
        let mut info = PROCESS_BASIC_INFORMATION::default();
        let status = NtQueryInformationProcess(
            handle,
            ProcessBasicInformation,
            (&mut info as *mut PROCESS_BASIC_INFORMATION).cast(),
            std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            std::ptr::null_mut(),
        );
        let born = creation_time(handle);
        let _ = CloseHandle(handle);
        if status.is_err() {
            return None;
        }
        Some(Process {
            identity: Identity { pid, born: born? },
            parent: u32::try_from(info.InheritedFromUniqueProcessId)
                .ok()
                .filter(|p| *p != 0),
        })
    }
}

#[cfg(windows)]
fn creation_time(handle: windows::Win32::Foundation::HANDLE) -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetProcessTimes;
    let (mut born, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the caller holds a valid process query handle and all outputs are initialized.
    unsafe {
        GetProcessTimes(handle, &mut born, &mut exit, &mut kernel, &mut user).ok()?;
    }
    let value = (u64::from(born.dwHighDateTime) << 32) | u64::from(born.dwLowDateTime);
    (value != 0).then_some(value)
}

#[cfg(target_os = "linux")]
fn process(pid: u32) -> Option<Process> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Parent and creation ticks come from one stat read. Field 22 follows comm, which
    // can contain spaces and ')'; field 4 is the parent PID.
    let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let born = fields.get(19)?.parse().ok()?;
    let parent = fields.get(1)?.parse::<u32>().ok().filter(|p| *p != 0);
    Some(Process {
        identity: Identity { pid, born },
        parent,
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn process(_pid: u32) -> Option<Process> {
    None
}

pub(super) fn identity(pid: u32) -> Option<Identity> {
    process(pid).map(|p| p.identity)
}

pub(super) fn table(sys: &sysinfo::System) -> Vec<Process> {
    let observed: Vec<_> = sys
        .processes()
        .keys()
        .filter_map(|p| process(p.as_u32()))
        .collect();
    // Validate AFTER all parent links were read. Otherwise a parent PID reused halfway
    // through enumeration could authenticate an unrelated replacement's new child.
    observed
        .into_iter()
        .filter(|p| identity(p.identity.pid) == Some(p.identity))
        .collect()
}

/// Validate identity on the SAME kernel handle used to terminate, avoiding PID reuse between
/// validation and termination. Never delegate this to taskkill or a bare PID kill.
#[cfg(windows)]
pub(super) fn terminate(expected: Identity) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    };
    // SAFETY: the owned handle is closed even if validation or termination fails.
    unsafe {
        let Ok(handle) = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
            false,
            expected.pid,
        ) else {
            return false;
        };
        let killed =
            creation_time(handle) == Some(expected.born) && TerminateProcess(handle, 1).is_ok();
        let _ = CloseHandle(handle);
        killed
    }
}

#[cfg(target_os = "linux")]
pub(super) fn terminate(expected: Identity) -> bool {
    // SAFETY: pidfd binds the signal to one process instance. Validate after opening it so PID
    // reuse cannot redirect the signal. Older kernels fail closed without bare-PID fallback.
    unsafe {
        let fd = libc::syscall(libc::SYS_pidfd_open, expected.pid, 0) as i32;
        if fd < 0 {
            return false;
        }
        let killed = identity(expected.pid) == Some(expected)
            && libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd,
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            ) == 0;
        libc::close(fd);
        killed
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(super) fn terminate(_expected: Identity) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    fn process(pid: u32, born: u64, parent: Option<u32>) -> Process {
        Process {
            identity: Identity { pid, born },
            parent,
        }
    }

    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn current_test_process_has_stable_read_only_identity() {
        let current = std::process::id();
        let first = super::process(current).expect("own process query must be available");
        assert_eq!(first.identity.pid, current);
        assert_ne!(first.identity.born, 0);
        assert_eq!(identity(current), Some(first.identity));
    }

    #[test]
    fn unrelated_new_interpreters_are_never_owned() {
        let root = process(10, 100, Some(1));
        let mut run = Ownership::new(root.identity);
        let live = run.observe(&[
            root,
            process(55376, 106, Some(48568)),
            process(111912, 106, Some(65276)),
        ]);
        assert_eq!(live, HashSet::from([root.identity]));
    }

    #[test]
    fn observed_descendant_remains_owned_after_parent_exit() {
        let root = process(10, 100, Some(1));
        let child = process(11, 101, Some(10));
        let grandchild = process(12, 102, Some(11));
        let mut run = Ownership::new(root.identity);
        assert_eq!(run.observe(&[grandchild, child, root]).len(), 3);
        assert_eq!(
            run.observe(&[grandchild]),
            HashSet::from([grandchild.identity])
        );
    }

    #[test]
    fn reused_root_or_child_pid_does_not_confer_ownership() {
        let root = process(10, 100, Some(1));
        let mut run = Ownership::new(root.identity);
        run.observe(&[root, process(11, 101, Some(10))]);
        assert!(
            run.observe(&[
                process(10, 200, Some(1)),
                process(11, 201, Some(10)),
                process(12, 202, Some(11))
            ])
            .is_empty()
        );
    }

    #[test]
    fn child_pid_reused_by_unrelated_parent_is_excluded_while_root_lives() {
        let root = process(10, 100_000_000, Some(1));
        let mut run = Ownership::new(root.identity);
        run.observe(&[root, process(11, 100_000_001, Some(10))]);
        // Creation identities differ within one second on Windows; second-resolution
        // start timestamps would fail to distinguish these process instances.
        let live = run.observe(&[
            root,
            process(11, 100_000_002, Some(99)),
            process(12, 100_000_003, Some(11)),
        ]);
        assert_eq!(live, HashSet::from([root.identity]));
    }

    #[test]
    fn unknown_or_older_child_cannot_be_adopted_through_stale_parent_pid() {
        let root = process(10, 100, Some(1));
        let mut run = Ownership::new(root.identity);
        assert_eq!(
            run.observe(&[root, process(11, 99, Some(10))]),
            HashSet::from([root.identity])
        );
        assert!(run.observe(&[process(12, 102, Some(10))]).is_empty());
    }
}

//! Gate the executor until it belongs to our process group/job. This closes the Windows
//! spawn/AssignProcessToJobObject race before the requested command can create descendants.
use serde_json::json;
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};
pub static CANCELLED: AtomicBool = AtomicBool::new(false);

pub fn worker(args: &[OsString]) -> i32 {
    let Some((directory, command)) = args.split_first() else {
        return 125;
    };
    let root = Path::new(directory);
    let mut gate = [0];
    if std::io::stdin().read_exact(&mut gate).is_err() || gate[0] != b'G' {
        return 125;
    }
    let Some((program, arguments)) = command.split_first() else {
        return 125;
    };
    let mut execution = Command::new(program);
    execution.args(arguments).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        execution.creation_flags(0x08000000);
    }
    let result = execution.spawn();
    let (value, exit) = match result {
        Ok(mut child) => match child.wait() {
            Ok(status) => {
                #[cfg(unix)]
                let signal = {
                    use std::os::unix::process::ExitStatusExt;
                    status.signal()
                };
                #[cfg(not(unix))]
                let signal: Option<i32> = None;
                #[cfg(unix)]
                let code = status.code().unwrap_or_else(|| 128 + signal.unwrap_or(1));
                #[cfg(not(unix))]
                let code = status.code().unwrap_or(125);
                (
                    json!({"exit_code":status.code(),"signal":signal,"pid":child.id()}),
                    code,
                )
            }
            Err(e) => (json!({"wait_error":e.to_string()}), 125),
        },
        Err(e) => (json!({"spawn_error":e.to_string()}), 127),
    };
    if super::write_json(&root.join("worker.json"), &value).is_err() {
        return 125;
    }
    exit
}

pub struct OwnedProcess {
    pub child: Child,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    stopped: bool,
}

impl OwnedProcess {
    pub fn spawn(command: &[OsString], root: &Path) -> Result<Self, String> {
        let mut launcher = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        launcher
            .arg("__kev_worker")
            .arg(root)
            .args(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            launcher.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            launcher.creation_flags(0x08000000 | 0x00000200);
        }
        let mut child = launcher
            .spawn()
            .map_err(|e| format!("executor launch failed: {e}"))?;
        #[cfg(windows)]
        let job = match attach_job(&child) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        if let Err(e) = child
            .stdin
            .take()
            .ok_or("executor gate missing")?
            .write_all(b"G")
        {
            let _ = child.kill();
            let _ = child.wait();
            #[cfg(windows)]
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
            return Err(format!("executor gate failed: {e}"));
        }
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
            stopped: false,
        })
    }
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 125);
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        self.stop();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

#[cfg(windows)]
fn attach_job(child: &Child) -> Result<windows_sys::Win32::Foundation::HANDLE, String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
    // SAFETY: initialized OS structures and a live Child handle; the owned handle is closed
    // on every error or by OwnedProcess::drop. The executor is waiting on its stdin gate.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        ) == 0
            || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0
        {
            let error = std::io::Error::last_os_error().to_string();
            CloseHandle(job);
            return Err(format!(
                "cannot contain executor; command not started: {error}"
            ));
        }
        Ok(job)
    }
}

pub fn install_cancel_handler() -> Result<(), String> {
    #[cfg(windows)]
    {
        unsafe extern "system" fn handler(event: u32) -> i32 {
            if matches!(event, 0..=2) {
                CANCELLED.store(true, Ordering::Relaxed);
                1
            } else {
                0
            }
        }
        // SAFETY: process-lifetime callback only writes a lock-free atomic flag.
        if unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(handler), 1) }
            == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    #[cfg(unix)]
    {
        extern "C" fn handler(_: i32) {
            CANCELLED.store(true, Ordering::Relaxed);
        }
        // SAFETY: signal handler performs only an atomic store; registered once by the CLI.
        unsafe {
            libc::signal(libc::SIGINT, handler as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, handler as *const () as libc::sighandler_t);
        }
    }
    Ok(())
}

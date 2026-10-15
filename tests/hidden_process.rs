//! Behavioral console-window check, not merely an assertion about a configured flag.
#[cfg(windows)]
#[test]
fn noninteractive_child_has_no_console_window() {
    let output = kritic::proc::hidden_command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "console_window_child_probe",
            "--ignored",
            "--nocapture",
        ])
        .env("KRITIC_HIDDEN_PROBE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[cfg(windows)]
#[test]
#[ignore = "executed by noninteractive_child_has_no_console_window in a hidden child"]
fn console_window_child_probe() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "GetConsoleWindow"]
        fn console_window() -> *mut std::ffi::c_void;
    }
    assert_eq!(std::env::var("KRITIC_HIDDEN_PROBE").unwrap(), "1");
    // GetConsoleWindow has no parameters and returns the calling process's console handle.
    assert!(unsafe { console_window() }.is_null());
}

//! Native process tools: hygiene (orphans, inherited sockets, leaks, leftovers) and the resource
//! sampler, against real processes. Needs `python` on PATH to make the misbehaving processes.

use std::path::Path;
use std::time::{Duration, Instant};

use kritic::native::hygiene::{audit, glob, leaks, roots_for, snapshot};
use kritic::native::resource::{resource, slope};
use serde_json::json;

fn python() -> Option<String> {
    ["python", "python3"]
        .into_iter()
        .find(|p| {
            std::process::Command::new(p)
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
        })
        .map(String::from)
}

// The parent opens an inheritable listening socket, starts a child that inherits it, and exits:
// the port stays bound by an exited owner, which is exactly what a real Unity app's engine did.
const PARENT: &str = r#"
import socket, subprocess, sys
s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(); s.set_inheritable(True)
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)", sys.argv[1]], close_fds=False)
open(sys.argv[2], "w").write(f"{s.getsockname()[1]} {child.pid}")
"#;

#[test]
#[cfg(windows)]
fn audit_finds_inherited_socket_and_orphan_then_cleans() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let result = dir.path().join("result.txt");
    let st = std::process::Command::new(&py)
        .args(["-c", PARENT])
        .arg(&marker)
        .arg(&result)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(st.success());
    let text = std::fs::read_to_string(&result).unwrap();
    let mut it = text.split_whitespace().map(|x| x.parse::<u32>().unwrap());
    let (port, child) = (it.next().unwrap(), it.next().unwrap());
    let roots = vec![dir.path().display().to_string()];
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut a = audit(&roots, &[], &[], false);
    while Instant::now() < deadline
        && !a["dead_owner_sockets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["port"] == port)
    {
        std::thread::sleep(Duration::from_millis(200));
        a = audit(&roots, &[], &[], false);
    }
    let sock = a["dead_owner_sockets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["port"] == port)
        .unwrap_or_else(|| panic!("no dead-owner socket for {port}: {a}"));
    assert!(
        sock["inheritors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["pid"] == child),
        "{sock}"
    );
    assert!(
        a["orphans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["pid"] == child),
        "{}",
        a["orphans"]
    );

    let cleaned = audit(&roots, &[], &[], true);
    assert!(
        cleaned["killed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k.as_str().unwrap().starts_with(&format!("{child} "))),
        "{}",
        cleaned["killed"]
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && std::net::TcpListener::bind(("127.0.0.1", port as u16)).is_err()
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", port as u16)).is_ok(),
        "the port is free again"
    );
}

#[test]
fn leaks_report_processes_started_during_a_run() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let dir = tempfile::tempdir().unwrap();
    let roots = vec![dir.path().display().to_string()];
    let before = snapshot(&roots);
    let mut child = std::process::Command::new(&py)
        .args(["-c", "import time; time.sleep(60)"])
        .arg(dir.path().join("x"))
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(700));
    let l = leaks(&before, true);
    assert!(
        l["processes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pid"] == child.id()),
        "{l}"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && child.try_wait().unwrap().is_none() {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        child.try_wait().unwrap().is_some(),
        "leaked process was killed"
    );
    // Nothing new after: an empty report.
    let before = snapshot(&roots);
    assert!(
        leaks(&before, false)["processes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn roots_cover_exe_command_and_cwd() {
    let exe = if cfg!(windows) {
        "C:/apps/demo/demo.exe"
    } else {
        "/opt/demo/demo"
    };
    let r = roots_for(Some(exe), None, Some("/work"), &["/extra".into()]);
    assert_eq!(r.len(), 3);
    assert!(Path::new(&r[0]).ends_with("demo"));
    let cmd = if cfg!(windows) {
        json!("\"C:/Program Files/tool/t.exe\" --flag")
    } else {
        json!("/usr/local/tool/t --flag")
    };
    let r = roots_for(None, Some(&cmd), None, &[]);
    assert!(r[0].replace('\\', "/").ends_with("tool"), "{r:?}");
    assert!(roots_for(None, Some(&json!(["relative", "x"])), None, &[]).is_empty());
}

#[test]
fn leftover_globs_match_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
    std::fs::write(dir.path().join("a/b/run.lock"), "").unwrap();
    std::fs::write(dir.path().join("a/keep.txt"), "").unwrap();
    let base = dir.path().display().to_string().replace('\\', "/");
    let found = glob(&format!("{base}/**/*.lock"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].ends_with("run.lock"));
    assert_eq!(glob(&format!("{base}/a/*.txt")).len(), 1);
    assert!(glob(&format!("{base}/none/*.x")).is_empty());
    let a = audit(&[base.clone()], &[], &[format!("{base}/**/*.lock")], false);
    assert_eq!(a["leftovers"].as_array().unwrap().len(), 1);
}

#[test]
fn slope_is_least_squares() {
    assert_eq!(slope(&[0.0, 1.0, 2.0], &[1.0, 3.0, 5.0]), 2.0);
    assert_eq!(slope(&[1.0], &[1.0]), 0.0);
}

#[tokio::test]
async fn resource_sampler_separates_a_leak_from_an_idle_process() {
    let Some(py) = python() else {
        return eprintln!("skipped: no python");
    };
    let leaky = json!([
        py,
        "-c",
        "import time\nx=[]\nwhile True:\n    x.append(bytearray(2000000)); time.sleep(0.05)"
    ]);
    let r = resource(Some(leaky), None, false, None, 4.0, 250, 0.5).await;
    assert_eq!(r["ok"], true, "{r}");
    let slope = r["stats"]["rss_slope_mb_per_min"].as_f64().unwrap();
    assert!(slope > 500.0, "a 40 MB/s leak measured as {slope} MB/min");
    assert!(r["stats"]["threads_peak"].as_u64().unwrap() >= 1 || !cfg!(windows));
    assert!(r["timeline"].as_array().unwrap().len() >= 5);

    let idle = json!([py, "-c", "import time\ntime.sleep(60)"]);
    let r = resource(Some(idle), None, false, None, 4.0, 250, 0.5).await;
    let slope = r["stats"]["rss_slope_mb_per_min"].as_f64().unwrap();
    assert!(slope.abs() < 5.0, "idle measured as {slope} MB/min");
    assert_eq!(r["stats"]["exited_early"], false);

    // A command that exits is reported as such; attaching needs permission.
    let quick = json!([python().unwrap(), "-c", "import time; time.sleep(1.5)"]);
    let r = resource(Some(quick), None, false, None, 6.0, 250, 0.2).await;
    assert_eq!(r["stats"]["exited_early"], true, "{r}");
    let r = resource(None, Some(std::process::id()), false, None, 1.0, 250, 0.0).await;
    assert!(r["error"].as_str().unwrap().contains("allow_attach"));
}

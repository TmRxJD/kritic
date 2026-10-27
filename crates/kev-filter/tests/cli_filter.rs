//! Real subprocess regressions: no Python, live model, network service or shell required.
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn root() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runs")
        .join(format!("test-{stamp}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn command(case: &str, root: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kev"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .env("KEV_TEST_CASE", case)
        .env("KEV_MARKER", root.join("marker"));
    cmd.args(["run", "--json", "--log-dir"])
        .arg(root)
        .args(["--timeout", "10"]);
    cmd
}
fn fixture_args() -> Vec<OsString> {
    vec![
        std::env::current_exe().unwrap().into_os_string(),
        "--exact".into(),
        "child_fixture".into(),
        "--ignored".into(),
        "--nocapture".into(),
    ]
}
fn result(case: &str, extra: &[&str]) -> (std::process::Output, Value, PathBuf) {
    let root = root();
    let output = command(case, &root)
        .args(extra)
        .arg("--")
        .args(fixture_args())
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output, report, root)
}
#[test]
#[ignore = "subprocess fixture, invoked explicitly by integration tests"]
fn child_fixture() {
    match std::env::var("KEV_TEST_CASE").unwrap().as_str() {
        "fail" => {
            println!("ROOT CAUSE: a missing fixture should remain visible");
            for _ in 0..5000 {
                println!("progress status: waiting for work");
            }
            eprintln!("ERROR: source mismatch\n  expected good\n  received bad");
            std::process::exit(7);
        }
        "bytes" => {
            std::io::stdout().write_all(b"raw\x00\xff\r\n").unwrap();
            eprintln!("warning: preserved stderr");
        }
        "argv" => println!(
            "ARGS={}",
            serde_json::to_string(
                &std::env::args_os()
                    .map(|v| v.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            )
            .unwrap()
        ),
        "exit124" => std::process::exit(124),
        "sleep" => {
            println!("STARTED");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(3));
            std::fs::write(std::env::var_os("KEV_MARKER").unwrap(), "alive").unwrap();
        }
        "flood" => loop {
            println!("a long output line that must be counted against the raw capture budget");
        },
        "candidate" => {
            for i in 0..12 {
                println!("record {i}: unique ordinary output for relevance ranking");
            }
            println!("\x1b[31mERROR: mandatory diagnostic\x1b[0m");
        }
        _ => panic!("unknown fixture"),
    }
}
#[test]
fn exit_failure_and_both_raw_streams_survive_filtering() {
    let (output, report, root) = result("fail", &[]);
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(report["child"]["exit_code"], 7);
    assert_eq!(report["capture_complete"], true);
    let dir = PathBuf::from(report["run_dir"].as_str().unwrap());
    let raw = std::fs::read_to_string(dir.join("stdout.log")).unwrap();
    let stderr = std::fs::read_to_string(dir.join("stderr.log")).unwrap();
    assert_eq!(raw.matches("progress status").count(), 5000);
    assert!(stderr.contains("expected good\n  received bad"));
    assert!(report.to_string().contains("ROOT CAUSE"));
    assert!(report.to_string().contains("ERROR: source mismatch"));
    let emitted = output.stdout.len() + output.stderr.len();
    let original = raw.len() + stderr.len();
    std::fs::write(root.join("benchmark.json"),serde_json::to_vec_pretty(&json!({"fixture":"5000 repeated records plus preserved failure","raw_bytes":original,"complete_emitted_bytes":emitted,"reduction_percent":100.0*(1.0-emitted as f64/original as f64),"child_exit_code":7,"kev_used":false,"scope":"synthetic byte counts, not billing/token counts or model-quality evidence"})).unwrap()).unwrap();
    assert!(
        emitted < original / 5,
        "actual complete JSON output must shrink this noisy fixture"
    );
}
#[test]
fn binary_logs_are_byte_exact_and_display_is_only_a_projection() {
    let (output, report, _) = result("bytes", &[]);
    assert!(output.status.success());
    let dir = PathBuf::from(report["run_dir"].as_str().unwrap());
    assert!(
        std::fs::read(dir.join("stdout.log"))
            .unwrap()
            .windows(7)
            .any(|w| w == b"raw\x00\xff\r\n")
    );
    assert!(
        report["excerpts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["lossy"] == true)
    );
}
#[test]
fn literal_arguments_are_not_reparsed_by_a_shell() {
    let root = root();
    let literal = "a space; $(not-a-command) & `literal`";
    let output = command("argv", &root)
        .arg("--")
        .args(fixture_args())
        .args(["--skip", literal])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let raw = std::fs::read_to_string(
        PathBuf::from(report["run_dir"].as_str().unwrap()).join("stdout.log"),
    )
    .unwrap();
    let line = raw
        .lines()
        .find_map(|l| l.split_once("ARGS=").map(|(_, v)| v))
        .unwrap();
    let args: Vec<String> = serde_json::from_str(line).unwrap();
    assert!(args.contains(&literal.to_string()));
}
#[test]
fn timeout_stops_an_actually_started_command() {
    let start = Instant::now();
    let (output, report, root) = result("sleep", &["--timeout", "1"]);
    assert_eq!(output.status.code(), Some(124));
    assert_eq!(report["termination"], "timeout");
    assert!(start.elapsed() < Duration::from_secs(4));
    let raw = std::fs::read_to_string(
        PathBuf::from(report["run_dir"].as_str().unwrap()).join("stdout.log"),
    )
    .unwrap();
    assert!(raw.contains("STARTED"));
    std::thread::sleep(Duration::from_secs(3));
    assert!(!root.join("marker").exists());
}
#[test]
fn log_cap_stops_flooding_and_never_claims_complete_capture() {
    let (output, report, _) = result("flood", &["--max-log-bytes", "2048"]);
    assert_eq!(output.status.code(), Some(125));
    assert_eq!(report["termination"], "log_limit");
    assert_eq!(report["capture_complete"], false);
    assert!(
        report["raw_bytes"]["stdout"].as_u64().unwrap()
            + report["raw_bytes"]["stderr"].as_u64().unwrap()
            <= 2048
    );
}
#[test]
fn invalid_remote_kev_endpoint_is_rejected_before_command_execution() {
    let root = root();
    let output = command("sleep", &root)
        .args(["--kev", "--kev-url", "http://example.com:8089/v1/decide"])
        .arg("--")
        .args(fixture_args())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&output.stderr).contains("loopback"));
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
}

#[test]
fn child_exit124_is_not_misreported_as_a_wrapper_timeout() {
    let (output, report, _) = result("exit124", &[]);
    assert_eq!(output.status.code(), Some(124));
    assert!(report["termination"].is_null());
    assert_eq!(report["child"]["exit_code"], 124);
}

#[test]
fn show_recovers_a_requested_stderr_line_from_the_saved_evidence() {
    let (_, report, _) = result("fail", &[]);
    let mut command = Command::new(env!("CARGO_BIN_EXE_kev"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command
        .arg("show")
        .arg(report["run_dir"].as_str().unwrap())
        .args(["--stream", "stderr", "--from", "2", "--lines", "1"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("[stderr:2]   expected good"));
    assert!(!text.contains("received bad"));
}

#[test]
fn nonexistent_command_has_a_spawn_error_not_a_successful_child_status() {
    let root = root();
    let output = command("unused", &root)
        .args(["--", "kev-deliberately-missing-executable-932125"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(127));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["child"]["spawn_error"].is_string());
    assert!(report["child"]["exit_code"].is_null());
}
#[test]
fn unavailable_kev_degrades_and_preserves_the_child_status() {
    let (output, report, _) = result(
        "candidate",
        &[
            "--kev",
            "--kev-url",
            "http://127.0.0.1:1/v1/decide",
            "--request-ms",
            "50",
            "--rank-ms",
            "100",
        ],
    );
    assert!(output.status.success());
    assert!(report["kev"]["error"].is_string());
    assert!(report.to_string().contains("ERROR: mandatory diagnostic"));
    assert!(!report.to_string().contains("\u{1b}"));
}
#[test]
fn utf8_spanning_a_long_line_fragment_is_not_falsely_lossy() {
    let text = format!("{}éZ\n", "a".repeat(4095));
    let mut frame = kev_filter::runner::filter::Framer::default();
    let mut records = frame.feed(0, &text.as_bytes()[..4096], false);
    records.extend(frame.feed(0, &text.as_bytes()[4096..], true));
    assert!(records.iter().all(|r| !r.lossy));
    assert_eq!(
        records.iter().map(|r| r.text.as_str()).collect::<String>(),
        text.trim_end()
    );
}

#[test]
fn local_kev_actually_ranks_candidates_but_cannot_hide_diagnostics() {
    use std::io::{BufRead, Read};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let finish = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = finish.clone();
    let requests = std::thread::spawn(move || {
        let mut calls = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !stop.load(std::sync::atomic::Ordering::Relaxed) && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
                    let mut line = String::new();
                    let mut length = 0;
                    loop {
                        line.clear();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" {
                            break;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            length = v.trim().parse::<usize>().unwrap();
                        }
                    }
                    assert!(length < 4096);
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let value: Value = serde_json::from_slice(&body).unwrap();
                    assert!(value["state"]["chunk"].is_string());
                    calls += 1;
                    let body =
                        json!({"answers":{"relevant":{"type":"noul","noul":0.0}}}).to_string();
                    write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("{e}"),
            }
        }
        calls
    });
    let endpoint = format!("http://{address}/v1/systemone");
    let (output, report, _) = result(
        "candidate",
        &["--kev", "--kev-url", &endpoint, "--rank-ms", "1000"],
    );
    finish.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(requests.join().unwrap() > 0);
    assert!(output.status.success());
    assert!(report["kev"]["scored_records"].as_u64().unwrap() > 0);
    assert!(
        report["selection"]["kev_rejected_candidates"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(report.to_string().contains("ERROR: mandatory diagnostic"));
    assert_eq!(report["kev"]["runtime_identity_verified"], false);
}

#[cfg(windows)]
#[test]
fn killing_wrapper_closes_job_and_stops_started_command() {
    let root = root();
    let mut child = command("sleep", &root)
        .arg("--")
        .args(fixture_args())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut started = false;
    while Instant::now() < deadline {
        for entry in std::fs::read_dir(&root).unwrap() {
            let log = entry.unwrap().path().join("stdout.log");
            if std::fs::read_to_string(log).is_ok_and(|s| s.contains("STARTED")) {
                started = true;
                break;
            }
        }
        if started {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(started);
    child.kill().unwrap();
    child.wait().unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert!(!root.join("marker").exists());
}

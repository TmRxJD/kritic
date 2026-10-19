#![cfg(windows)]

use kritic::checks::{Status, run_command};

#[tokio::test]
async fn quoted_program_path_preserves_argument_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("validator with spaces.cmd");
    std::fs::write(
        &script,
        "@echo off\r\necho first=%~1\r\necho second=%~2\r\n",
    )
    .unwrap();
    let command = format!("\"{}\" alpha \"second argument\"", script.display());
    let result = run_command(&command, dir.path(), 10).await;
    assert_eq!(result.status, Status::Pass, "{}", result.output_tail);
    assert!(result.output_tail.contains("first=alpha"));
    assert!(result.output_tail.contains("second=second argument"));
}

#[tokio::test]
async fn quoted_program_path_keeps_shell_failure_status() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("validator with spaces.cmd");
    std::fs::write(&script, "@echo off\r\nexit /b 7\r\n").unwrap();
    let command = format!("\"{}\" && echo must-not-run", script.display());
    let result = run_command(&command, dir.path(), 10).await;
    assert_eq!(result.status, Status::Fail);
    assert_eq!(result.exit_code, Some(7));
    assert!(!result.output_tail.contains("must-not-run"));
}

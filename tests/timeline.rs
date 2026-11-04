use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Command, Output},
};

fn run(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Output {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.output().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn capture(path: &Path, filter: &str) {
    assert_success(&run(
        "ffmpeg",
        &[
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=black:size=32x32:rate=25:duration=0.2",
            "-vf",
            filter,
            "-fps_mode",
            "passthrough",
            "-c:v",
            "ffv1",
            "-threads",
            "1",
            path.to_str().unwrap(),
        ],
    ));
}

#[test]
#[ignore = "requires ffmpeg and ffprobe on PATH; run explicitly with --ignored"]
fn timeline_preserves_vfr_frames_and_correlates_an_actual_pixel_change() {
    let temp = tempfile::tempdir().unwrap();
    let baseline = temp.path().join("baseline.mkv");
    let candidate = temp.path().join("candidate.mkv");
    let telemetry = temp.path().join("candidate.json");
    capture(&baseline, "null");
    capture(
        &candidate,
        "drawbox=color=white:t=fill:enable='eq(n,3)',select='not(eq(n,2))'",
    );
    std::fs::write(&telemetry, json!({
        "video_first_frame_us":0,
        "runtime_frames":{"source":"controlled test intervals","samples":[
            {"at_us":40000,"frame_time_us":16000},{"at_us":120000,"frame_time_us":60000}
        ]},
        "workers":{"source":"controlled test spans","coverage_start_us":0,"coverage_end_us":160000,
            "spans":{"geometry":[{"start_us":90000,"end_us":112000}],
                "preframe":[{"start_us":45000,"end_us":55000}]}},
        "resources":{"source":"controlled test samples","scope":"fixture",
            "samples":[{"at_us":110000,"cpu_pct":87.5,"rss_bytes":123456}]}
    }).to_string()).unwrap();

    let output = run(
        env!("CARGO_BIN_EXE_kritic"),
        &[
            "timeline",
            "compare",
            "--baseline",
            baseline.to_str().unwrap(),
            "--candidate",
            candidate.to_str().unwrap(),
            "--candidate-telemetry",
            telemetry.to_str().unwrap(),
            "--match-us",
            "0",
            "--resource-age-us",
            "20000",
        ],
    );
    assert_success(&output);
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[0]["candidate"]["frame_count"], 4);
    assert_eq!(rows[0]["candidate"]["video_cadence"]["avg_fps"], 18.75);
    assert_eq!(
        rows[0]["candidate"]["runtime_timing"]["stats"]["stutters"],
        1
    );
    assert_eq!(rows[1]["visual"]["exact_match"], true);
    assert_eq!(rows[3]["baseline"]["at_us"], 80000);
    assert!(rows[3]["candidate"].is_null());
    let hitch = &rows[4];
    assert_eq!(hitch["baseline"]["at_us"], 120000);
    assert_eq!(hitch["candidate"]["video_interval_us"], 80000);
    assert_eq!(hitch["visual"]["changed_pixel_fraction"], 1.0);
    assert_eq!(
        hitch["candidate"]["previous_frame_visual"]["changed_pixel_fraction"],
        1.0
    );
    assert_eq!(
        hitch["candidate"]["telemetry"]["runtime_frame"]["stutter"],
        true
    );
    assert_eq!(
        hitch["candidate"]["telemetry"]["worker_busy_us"]["geometry"],
        22000
    );
    assert_eq!(
        hitch["candidate"]["telemetry"]["worker_busy_us"]["preframe"],
        10000
    );
    assert!(
        hitch["candidate"]["telemetry"]["runtime_frame"]["worker_busy_us"]
            .get("preframe")
            .is_none()
    );
    assert_eq!(
        hitch["candidate"]["telemetry"]["resources"]["age_us"],
        10000
    );
    assert_eq!(
        hitch["candidate"]["telemetry"]["resources"]["sample"]["cpu_pct"],
        87.5
    );
    assert!(hitch["baseline"]["telemetry"]["runtime_frame"].is_null());
    let last = &rows[5]["candidate"]["telemetry"];
    assert!(last["resources"].is_null());
    assert!(last["runtime_frame"].is_null());
    assert_eq!(last["worker_busy_us"], json!({}));
    assert_eq!(
        rows[6],
        json!({"type":"complete","matched_frames":4,"baseline_only_frames":1,
        "candidate_only_frames":0,"visual_status":"compared","visual_comparisons":4})
    );
}

#[test]
fn overlapping_worker_intervals_are_rejected_before_video_decode() {
    let temp = tempfile::tempdir().unwrap();
    let telemetry = temp.path().join("bad.json");
    std::fs::write(
        &telemetry,
        json!({
            "video_first_frame_us":0,
            "workers":{"source":"fixture","coverage_start_us":0,"coverage_end_us":10,"spans":{
                "physics":[{"start_us":1,"end_us":5},{"start_us":4,"end_us":7}]
            }}
        })
        .to_string(),
    )
    .unwrap();
    let output = run(
        env!("CARGO_BIN_EXE_kritic"),
        &[
            "timeline",
            "compare",
            "--baseline",
            "nonexistent-a.mkv",
            "--candidate",
            "nonexistent-b.mkv",
            "--baseline-telemetry",
            telemetry.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("overlapping worker spans for physics")
    );
}

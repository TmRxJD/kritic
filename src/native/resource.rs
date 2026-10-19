//! Resource sampling of a process tree: CPU, memory, handles, threads, disk I/O and whole-GPU
//! use over a time window. Facts only; budgets are judged in `phase3`.

use std::collections::HashSet;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::procs::{self, pid};

#[derive(Debug, Clone, Copy, Default)]
struct Sample {
    t: f64,
    cpu: f64,
    rss: f64,
    private: f64,
    handles: f64,
    threads: f64,
    read: f64,
    write: f64,
    procs: f64,
}

fn gpu_sample() -> Option<(f64, f64)> {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu,memory.used",
            "--format=csv,noheader,nounits",
        ])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .to_string();
    let (u, m) = line.split_once(',')?;
    Some((u.trim().parse().ok()?, m.trim().parse().ok()?))
}

/// Least-squares slope of `ys` over `xs`.
pub fn slope(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    if xs.len() < 2 {
        return 0.0;
    }
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let den: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    if den == 0.0 {
        0.0
    } else {
        xs.iter()
            .zip(ys)
            .map(|(x, y)| (x - mx) * (y - my))
            .sum::<f64>()
            / den
    }
}

fn round(v: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (v * f).round() / f
}

fn sample_tree(
    sys: &mut sysinfo::System,
    root: u32,
    cum_disk: &mut std::collections::HashMap<u32, (u64, u64)>,
) -> (Sample, HashSet<u32>) {
    procs::refresh(sys);
    let pids = procs::tree_pids(sys, root);
    let threads = procs::thread_counts();
    let mut s = Sample {
        procs: pids.len() as f64,
        ..Default::default()
    };
    for p in &pids {
        let Some(proc_) = sys.process(pid(*p)) else {
            continue;
        };
        s.cpu += proc_.cpu_usage() as f64;
        s.rss += proc_.memory() as f64;
        let x = procs::extra(*p, &threads);
        s.private += if x.private_bytes > 0 {
            x.private_bytes as f64
        } else {
            proc_.memory() as f64
        };
        s.handles += x.handles as f64;
        s.threads += x.threads as f64;
        let du = proc_.disk_usage();
        let e = cum_disk.entry(*p).or_insert((0, 0));
        e.0 = e.0.max(du.total_read_bytes);
        e.1 = e.1.max(du.total_written_bytes);
    }
    // Disk counters are cumulative per process; count processes that already exited too.
    s.read = cum_disk.values().map(|v| v.0 as f64).sum();
    s.write = cum_disk.values().map(|v| v.1 as f64).sum();
    (s, pids)
}

fn summarize(samples: &[Sample], gpu: &[(f64, f64)], exit_code: Option<i32>) -> Value {
    let mb = 1024.0 * 1024.0;
    let ts: Vec<f64> = samples.iter().map(|s| s.t).collect();
    let mut cpus: Vec<f64> = samples.iter().map(|s| s.cpu).collect();
    cpus.sort_by(f64::total_cmp);
    let rss: Vec<f64> = samples.iter().map(|s| s.rss / mb).collect();
    let span = (ts[ts.len() - 1] - ts[0]).max(1e-6);
    let (first, last) = (samples[0], samples[samples.len() - 1]);
    let ncpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1) as f64;
    let avg = cpus.iter().sum::<f64>() / cpus.len() as f64;
    json!({
        "duration_s": round(ts[ts.len() - 1], 1),
        "samples": samples.len(),
        "cpu_cores": ncpu as u64,
        "cpu_avg_pct": round(avg, 1),
        "cpu_p95_pct": round(cpus[((cpus.len() - 1) as f64 * 0.95) as usize], 1),
        "cpu_max_pct": round(cpus[cpus.len() - 1], 1),
        "cpu_avg_pct_of_machine": round(avg / ncpu, 1),
        "rss_start_mb": round(rss[0], 1),
        "rss_end_mb": round(rss[rss.len() - 1], 1),
        "rss_peak_mb": round(rss.iter().copied().fold(0.0, f64::max), 1),
        "rss_slope_mb_per_min": round(slope(&ts, &rss) * 60.0, 2),
        "private_end_mb": round(last.private / mb, 1),
        "handles_start": first.handles as u64,
        "handles_end": last.handles as u64,
        "threads_peak": samples.iter().map(|s| s.threads as u64).max().unwrap_or(0),
        "procs_peak": samples.iter().map(|s| s.procs as u64).max().unwrap_or(0),
        "read_mb": round((last.read - first.read) / mb, 2),
        "write_mb": round((last.write - first.write) / mb, 2),
        "read_mb_per_s": round((last.read - first.read) / mb / span, 3),
        "write_mb_per_s": round((last.write - first.write) / mb / span, 3),
        "gpu_util_avg_pct": if gpu.is_empty() { Value::Null } else { json!(round(gpu.iter().map(|g| g.0).sum::<f64>() / gpu.len() as f64, 1)) },
        "gpu_util_max_pct": gpu.iter().map(|g| g.0).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v)))),
        "gpu_mem_delta_mb": if gpu.len() > 1 { json!((gpu[gpu.len() - 1].1 - gpu[0].1).round()) } else { Value::Null },
        "exited_early": exit_code.is_some(),
        "exit_code": exit_code,
    })
}

/// Start `command` (a string runs through the shell, like artifact_validate's commands; an
/// argv array runs directly) or attach to `pid`.
fn start(
    command: Option<&Value>,
    attach: Option<u32>,
    allow_attach: bool,
    cwd: Option<&str>,
) -> Result<(u32, Option<std::process::Child>), String> {
    if let Some(p) = attach {
        if !allow_attach {
            return Err("attaching to an existing pid requires allow_attach".into());
        }
        return Ok((p, None));
    }
    let mut cmd = match command {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            let mut c = if cfg!(windows) {
                std::process::Command::new("cmd")
            } else {
                std::process::Command::new("sh")
            };
            c.args(if cfg!(windows) {
                ["/C", s.as_str()]
            } else {
                ["-c", s.as_str()]
            });
            c
        }
        Some(Value::Array(a)) if !a.is_empty() => {
            let argv: Vec<String> = a
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(String::from)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect();
            let mut c = std::process::Command::new(&argv[0]);
            c.args(&argv[1..]);
            c
        }
        _ => return Err("give command or pid".into()),
    };
    if let Some(d) = cwd.filter(|d| !d.is_empty()) {
        cmd.current_dir(d);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = cmd
        .spawn()
        .map_err(|e| format!("cannot start command: {e}"))?;
    Ok((child.id(), Some(child)))
}

/// Sample the process tree for `duration_s` after a `warmup_s` settle.
pub async fn resource(
    command: Option<Value>,
    attach: Option<u32>,
    allow_attach: bool,
    cwd: Option<String>,
    duration_s: f64,
    interval_ms: u64,
    warmup_s: f64,
) -> Value {
    tokio::task::spawn_blocking(move || {
        resource_blocking(
            command.as_ref(),
            attach,
            allow_attach,
            cwd.as_deref(),
            duration_s,
            interval_ms,
            warmup_s,
        )
    })
    .await
    .unwrap_or_else(|e| json!({"ok": false, "error": format!("sampler crashed: {e}")}))
}

fn resource_blocking(
    command: Option<&Value>,
    attach: Option<u32>,
    allow_attach: bool,
    cwd: Option<&str>,
    duration_s: f64,
    interval_ms: u64,
    warmup_s: f64,
) -> Value {
    let (root, mut child) = match start(command, attach, allow_attach, cwd) {
        Ok(x) => x,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let interval = Duration::from_millis(interval_ms.max(100));
    let mut sys = procs::system();
    let mut cum_disk = std::collections::HashMap::new();
    let mut samples: Vec<Sample> = Vec::new();
    let mut gpu = Vec::new();
    let mut exit_code = None;
    let t0 = Instant::now();
    let mut n = 0usize;
    while t0.elapsed().as_secs_f64() < duration_s + warmup_s {
        let (mut s, tree) = sample_tree(&mut sys, root, &mut cum_disk);
        let t = t0.elapsed().as_secs_f64();
        if t >= warmup_s {
            s.t = round(t - warmup_s, 2);
            samples.push(s);
            n += 1;
            if n.is_multiple_of((1000 / interval.as_millis().max(1) as usize).max(1))
                && let Some(g) = gpu_sample()
            {
                gpu.push(g);
            }
        }
        if let Some(c) = child.as_mut()
            && let Ok(Some(status)) = c.try_wait()
            && tree.len() <= 1
        {
            exit_code = Some(status.code().unwrap_or(-1));
            break;
        }
        std::thread::sleep(interval);
    }
    if let Some(c) = child.as_mut()
        && c.try_wait().ok().flatten().is_none()
    {
        procs::refresh(&mut sys);
        procs::kill_all(&sys, &procs::tree_pids(&sys, root));
        let _ = c.kill();
        let _ = c.wait();
    }
    if samples.is_empty() {
        return json!({"ok": false, "error": "no samples (process exited immediately?)", "exit_code": exit_code});
    }
    let step = (samples.len() / 60).max(1);
    let mb = 1024.0 * 1024.0;
    let timeline: Vec<Value> = samples
        .iter()
        .step_by(step)
        .map(|s| json!({"t": s.t, "cpu": round(s.cpu, 1), "rss_mb": round(s.rss / mb, 1)}))
        .collect();
    json!({"ok": true, "stats": summarize(&samples, &gpu, exit_code), "timeline": timeline, "gpu_note": "whole-GPU (nvidia-smi), not per-process"})
}

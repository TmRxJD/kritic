//! Shared mode keeps one model on the GPU at a time. With Kev (laya, ~4-5 GB) and Qwen2.5-VL (Ollama, ~5 GB) both
//! resident next to a desktop on a 12 GB card, Windows spills VRAM to system memory and vision
//! calls go from ~1.3 s to ~66 s (measured). So: unload the vision model before Kev work, stop
//! laya before vision work. Explicit CPU-only Kev is excluded from GPU coordination and cleanup.
//! A validated pair of dedicated GPU UUIDs uses independent leases and never cross-evicts.

use std::time::Duration;

use serde_json::json;

use crate::config::Config;

/// Deterministic checks never own the GPU; vision always does, even with CPU Kev.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workload {
    Deterministic,
    Kev,
    Vision,
}

pub fn requires_gpu(cpu_only: bool, workload: Workload) -> bool {
    match workload {
        Workload::Deterministic => false,
        Workload::Kev => !cpu_only,
        Workload::Vision => true,
    }
}

/// Older health endpoints omit the device; an explicit non-CPU report must never be ignored.
pub fn validate_cpu_health(cpu_only: bool, health: &serde_json::Value) -> Result<(), String> {
    if !cpu_only {
        return Ok(());
    }
    for pointer in [
        "/device",
        "/device/type",
        "/device/kind",
        "/model/device",
        "/runtime/device",
    ] {
        if let Some(device) = health.pointer(pointer).and_then(serde_json::Value::as_str)
            && !device.eq_ignore_ascii_case("cpu")
        {
            return Err(format!(
                "kev.cpu_only=true but laya reports device {device:?}; request refused. Restart the service on CPU or disable cpu_only to use GPU coordination; no process was stopped"
            ));
        }
    }
    Ok(())
}

/// Cross-process GPU lock: an OS file lock (LockFileEx on Windows) on `<runs_dir>/gpu.lock`,
/// released automatically when the holder exits or crashes. Every kritic process (one per Claude
/// Code session) takes it around model work. Dedicated mode keys leases by physical UUID.
pub struct GpuFileLock {
    file: std::fs::File,
}

impl Drop for GpuFileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub async fn lock_across_processes(cfg: &Config, tool: &str) -> Result<GpuFileLock, String> {
    cfg.validate_gpu_placement()?;
    std::fs::create_dir_all(&cfg.runs_dir).map_err(|e| e.to_string())?;
    let stem = if cfg.dedicated_gpus() {
        if matches!(tool, "visual_critic" | "ux_critic") {
            cfg.vision_gpu_uuid.as_ref().unwrap().to_ascii_lowercase()
        } else {
            cfg.kev_gpu_uuid.as_ref().unwrap().to_ascii_lowercase()
        }
    } else {
        "gpu".into()
    };
    let path = cfg.runs_dir.join(format!("{stem}.lock"));
    let owner = cfg.runs_dir.join(format!("{stem}.owner"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(cfg.gpu_wait_s);
    let mut announced = false;
    loop {
        match file.try_lock() {
            Ok(()) => {
                let _ = std::fs::write(&owner, format!("{} {tool}", std::process::id()));
                return Ok(GpuFileLock { file });
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if !announced {
                    let holder = std::fs::read_to_string(&owner).unwrap_or_default();
                    crate::events::ready(
                        "gpu",
                        "waiting",
                        &format!("held by another session: {holder}"),
                    );
                    announced = true;
                }
                if std::time::Instant::now() > deadline {
                    return Err(format!(
                        "GPU busy in another session for over {}s",
                        cfg.gpu_wait_s
                    ));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(format!("gpu lock: {e}")),
        }
    }
}

/// Ask Ollama to evict `model` now (keep_alive 0). No-op if Ollama isn't running.
pub async fn unload_vision(cfg: &Config) {
    if cfg.kev_cpu_only || cfg.kev_gpu_uuid.is_some() || cfg.vision_gpu_uuid.is_some() {
        return;
    }
    let http = reqwest::Client::new();
    crate::events::ready("vision", "unloaded", "GPU handed to Kev");
    let _ = http
        .post(format!("{}/api/generate", cfg.ollama_url))
        .json(&json!({"model": cfg.vision_model, "keep_alive": 0}))
        .timeout(Duration::from_secs(10))
        .send()
        .await;
}

/// Ollama's `/api/ps` body lists no loaded model.
pub fn ps_idle(v: &serde_json::Value) -> bool {
    v.get("models")
        .and_then(serde_json::Value::as_array)
        .is_none_or(|a| a.is_empty())
}

/// A GPU router's `/v1/lanes` body: every lane (or only the lane on `gpu_uuid`) has no job
/// running and no model resident. None when the body is not a lane list.
pub fn lanes_idle(v: &serde_json::Value, gpu_uuid: Option<&str>) -> Option<bool> {
    let lanes = v.as_array()?;
    let on_gpu = |l: &&serde_json::Value| {
        gpu_uuid.is_none_or(|u| {
            l.pointer("/gpu/uuid")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|x| x.eq_ignore_ascii_case(u))
        })
    };
    Some(lanes.iter().filter(on_gpu).all(|l| {
        l.get("busy").is_none_or(serde_json::Value::is_null)
            && l.get("resident_model")
                .is_none_or(serde_json::Value::is_null)
    }))
}

/// The vision GPU is free for Kev: Ollama (`/api/ps`) has no model loaded, or a GPU router
/// (`/v1/lanes`) shows its lane idle, or nothing answers at `vision.ollama_url`.
pub async fn vision_idle(cfg: &Config) -> bool {
    let http = reqwest::Client::new();
    let get = |path: &str| {
        http.get(format!("{}{path}", cfg.ollama_url))
            .timeout(Duration::from_secs(2))
            .send()
    };
    let json = |r: reqwest::Response| async move {
        if r.status().is_success() {
            r.json::<serde_json::Value>().await.ok()
        } else {
            None
        }
    };
    let ps = match get("/api/ps").await {
        Err(_) => return true,
        Ok(r) => json(r).await,
    };
    if let Some(v) = ps {
        return ps_idle(&v);
    }
    match get("/v1/lanes").await {
        Ok(r) => json(r)
            .await
            .and_then(|v| lanes_idle(&v, cfg.vision_gpu_kev_uuid.as_deref()))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Run a configured shell command with no window and wait for it, bounded by `limit`.
pub async fn run_hidden(cmd: &str, limit: Duration) -> Result<(), String> {
    let mut c = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let status = tokio::time::timeout(limit, c.status())
        .await
        .map_err(|_| format!("`{cmd}` did not return within {}s", limit.as_secs()))?
        .map_err(|e| format!("`{cmd}` failed to run: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{cmd}` exited with {status}"))
    }
}

async fn answers(url: &str) -> bool {
    reqwest::Client::new()
        .get(format!("{url}/health"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

/// Before vision work: stop the Kev that runs on the vision GPU (`kev.vision_gpu_laya_url`) so
/// the vision model gets its memory. Returns whether it was stopped.
pub async fn free_vision_gpu(cfg: &Config) -> bool {
    let (Some(url), Some(stop)) = (&cfg.vision_gpu_kev_url, &cfg.vision_gpu_kev_stop) else {
        return false;
    };
    if !answers(url).await {
        return false;
    }
    if let Err(e) = run_hidden(stop, Duration::from_secs(60)).await {
        crate::events::ready("laya", "vision-gpu stop failed", &e);
        return false;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while answers(url).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    crate::events::ready("laya", "vision-gpu stopped", "GPU handed to vision");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    true
}

/// Stop laya so the vision model gets the GPU. Returns whether a process was stopped.
pub async fn stop_laya(cfg: &Config) -> bool {
    // CPU services may be shared launchers; GPU cleanup must never kill them by image name.
    if cfg.kev_cpu_only
        || cfg.kev_externally_managed
        || cfg.kev_gpu_uuid.is_some()
        || cfg.vision_gpu_uuid.is_some()
    {
        return false;
    }
    let exe = cfg
        .laya_exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "laya.exe".into());
    let out = if cfg!(windows) {
        tokio::process::Command::new("taskkill")
            .args(["/F", "/IM", &exe])
            .output()
            .await
    } else {
        tokio::process::Command::new("pkill")
            .args(["-f", &exe])
            .output()
            .await
    };
    let stopped = out.map(|o| o.status.success()).unwrap_or(false);
    if stopped {
        crate::events::ready("laya", "stopped", "GPU handed to vision");
    }
    if stopped {
        // give the driver a moment to release VRAM
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    stopped
}

#[cfg(test)]
mod tests {
    fn dedicated_config() -> crate::config::Config {
        let mut cfg = crate::config::Config::load();
        cfg.kev_cpu_only = false;
        cfg.kev_gpu_uuid = Some("GPU-11111111-1111-1111-1111-111111111111".into());
        cfg.vision_gpu_uuid = Some("GPU-22222222-2222-2222-2222-222222222222".into());
        cfg.laya_device = "vulkan:0".into();
        cfg.vulkan_visible_devices = Some("1".into());
        cfg
    }

    #[test]
    fn dedicated_placement_rejects_partial_same_and_ambiguous_devices() {
        let mut cfg = dedicated_config();
        assert!(cfg.validate_gpu_placement().is_ok());
        cfg.vision_gpu_uuid = cfg.kev_gpu_uuid.clone();
        assert!(cfg.validate_gpu_placement().is_err());
        cfg.vision_gpu_uuid = None;
        assert!(cfg.validate_gpu_placement().is_err());
        cfg = dedicated_config();
        cfg.laya_device = "auto".into();
        assert!(cfg.validate_gpu_placement().is_err());
        cfg = dedicated_config();
        cfg.vulkan_visible_devices = None;
        assert!(cfg.validate_gpu_placement().is_err());
        cfg = dedicated_config();
        cfg.kev_cpu_only = true;
        assert!(cfg.validate_gpu_placement().is_err());
    }

    #[test]
    fn dedicated_health_requires_matching_backend() {
        let cfg = dedicated_config();
        let client = crate::kev::KevClient::new(std::sync::Arc::new(cfg));
        assert!(
            client
                .validate_health(&serde_json::json!({"status":"ok"}))
                .is_err()
        );
        assert!(
            client
                .validate_health(&serde_json::json!({"device":"cuda:0"}))
                .is_err()
        );
        assert!(
            client
                .validate_health(&serde_json::json!({"device":"vulkan:1"}))
                .is_err()
        );
        assert!(
            client
                .validate_health(&serde_json::json!({"device":"vulkan:0"}))
                .is_ok()
        );
    }

    #[test]
    fn vulkan_launch_pins_verified_ordinal_without_cuda_graphs() {
        let mut cfg = dedicated_config();
        cfg.laya_max_batch = Some(4);
        let cmd = crate::kev::laya_command(&cfg);
        let args: Vec<_> = cmd
            .get_args()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        assert!(args.windows(2).any(|v| v == ["--device", "vulkan:0"]));
        assert!(args.windows(2).any(|v| v == ["--max-batch", "4"]));
        assert!(!args.iter().any(|v| v == "--cuda-graph"));
        let env: std::collections::BTreeMap<_, _> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            env.get("GGML_VK_VISIBLE_DEVICES").unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(env.get("CUDA_VISIBLE_DEVICES").unwrap(), &cfg.kev_gpu_uuid);
        cfg.kev_gpu_uuid = None;
        cfg.vision_gpu_uuid = None;
        cfg.vulkan_visible_devices = None;
        cfg.laya_device = "auto".into();
        assert!(
            crate::kev::laya_command(&cfg)
                .get_args()
                .any(|v| v == "--cuda-graph")
        );
        cfg.kev_cpu_only = true;
        assert!(
            !crate::kev::laya_command(&cfg)
                .get_args()
                .any(|v| v == "--cuda-graph")
        );
    }

    #[tokio::test]
    async fn dedicated_models_have_independent_cross_process_leases() {
        let mut cfg = dedicated_config();
        let dir = tempfile::tempdir().unwrap();
        cfg.runs_dir = dir.path().into();
        let _kev = super::lock_across_processes(&cfg, "critic_score")
            .await
            .unwrap();
        let _vision = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::lock_across_processes(&cfg, "visual_critic"),
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[tokio::test]
    async fn dedicated_vision_refuses_to_launch_a_generic_server() {
        let mut cfg = dedicated_config();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        cfg.ollama_url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        cfg.ollama_exe = "kritic-test-nonexistent-ollama.exe".into();
        let error = crate::vision::ensure_ollama(&reqwest::Client::new(), &cfg)
            .await
            .unwrap_err();
        assert!(error.starts_with("externally managed Ollama unreachable"));
    }

    #[tokio::test]
    async fn externally_managed_kev_never_autostarts_or_stops_a_service() {
        let mut cfg = dedicated_config();
        cfg.kev_externally_managed = true;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        cfg.laya_url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        cfg.laya_exe = "kritic-test-nonexistent-external-service.exe".into();
        assert!(!super::stop_laya(&cfg).await);
        let client = crate::kev::KevClient::new(std::sync::Arc::new(cfg));
        assert!(
            client
                .ensure_up()
                .await
                .unwrap_err()
                .starts_with("externally managed laya unreachable")
        );
    }

    #[tokio::test]
    async fn dedicated_cleanup_neither_evicts_nor_stops_other_models() {
        let mut cfg = dedicated_config();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        cfg.ollama_url = format!("http://{}", listener.local_addr().unwrap());
        cfg.laya_exe = "kritic-test-nonexistent-dedicated-service.exe".into();
        assert!(!super::stop_laya(&cfg).await);
        super::unload_vision(&cfg).await;
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn explicit_gpu_health_is_rejected_in_cpu_mode() {
        for device in ["cuda", "cuda:0", "metal", "vulkan"] {
            let health = serde_json::json!({"device":device});
            assert!(super::validate_cpu_health(true, &health).is_err());
            assert!(super::validate_cpu_health(false, &health).is_ok());
        }
        assert!(super::validate_cpu_health(true, &serde_json::json!({"device":"cpu"})).is_ok());
        assert!(super::validate_cpu_health(true, &serde_json::json!({"status":"ok"})).is_ok());
    }

    #[tokio::test]
    async fn cpu_cleanup_neither_stops_a_process_nor_contacts_vision() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut cfg = crate::config::Config::load();
        cfg.kev_cpu_only = true;
        cfg.laya_exe = "kritic-test-nonexistent-cpu-service.exe".into();
        cfg.ollama_url = format!("http://{}", listener.local_addr().unwrap());
        assert!(
            !tokio::time::timeout(
                std::time::Duration::from_millis(100),
                super::stop_laya(&cfg)
            )
            .await
            .expect("CPU cleanup must return without process control")
        );
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            super::unload_vision(&cfg),
        )
        .await
        .expect("CPU cleanup must not contact vision");
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

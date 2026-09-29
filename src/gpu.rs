//! One model on the GPU at a time. With Kev (laya, ~4-5 GB) and Qwen2.5-VL (Ollama, ~5 GB) both
//! resident next to a desktop on a 12 GB card, Windows spills VRAM to system memory and vision
//! calls go from ~1.3 s to ~66 s (measured). So: unload the vision model before Kev work, stop
//! laya before vision work. laya restarts in ~13 s with the CUDA JIT cache.

use std::time::Duration;

use serde_json::json;

use crate::config::Config;

/// Cross-process GPU lock: an OS file lock (LockFileEx on Windows) on `<runs_dir>/gpu.lock`,
/// released automatically when the holder exits or crashes. Every kritic process (one per Claude
/// Code session) takes it around model work, so two sessions never load Kev and vision together.
pub struct GpuFileLock {
    file: std::fs::File,
}

impl Drop for GpuFileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub async fn lock_across_processes(cfg: &Config, tool: &str) -> Result<GpuFileLock, String> {
    std::fs::create_dir_all(&cfg.runs_dir).map_err(|e| e.to_string())?;
    let path = cfg.runs_dir.join("gpu.lock");
    let owner = cfg.runs_dir.join("gpu.owner");
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
    let http = reqwest::Client::new();
    crate::events::ready("vision", "unloaded", "GPU handed to Kev");
    let _ = http
        .post(format!("{}/api/generate", cfg.ollama_url))
        .json(&json!({"model": cfg.vision_model, "keep_alive": 0}))
        .timeout(Duration::from_secs(10))
        .send()
        .await;
}

/// Stop laya so the vision model gets the GPU. Returns whether a process was stopped.
pub async fn stop_laya(cfg: &Config) -> bool {
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

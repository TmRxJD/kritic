//! Cooperative, expiring memory leases on an already-running local Kev server.
//! This module never starts/stops services or loads a model to discover support.

use crate::config::Config;
use reqwest::{Client, Url};
use serde_json::{Value, json};
use std::{
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

struct LeaseRequest {
    http: Client,
    base: Url,
    id: String,
    owner: String,
    work_deadline: tokio::time::Instant,
}

/// A lease is armed before its reservation request is sent, so cancellation
/// also releases a reservation whose HTTP acknowledgement was lost. The native
/// server's cancellation tombstone prevents a late reserve resurrecting it.
pub struct MemoryLease {
    request: Option<LeaseRequest>,
}

fn local_base(raw: &str) -> Option<Url> {
    let mut url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return None;
    }
    let host = url.host_str()?;
    if host != "localhost"
        && !host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
    {
        return None;
    }
    // Pin localhost too: do not rely on DNS for this loopback-only control API.
    if url.host_str() == Some("localhost") {
        url.set_host(Some("127.0.0.1")).ok()?;
    }
    Some(url)
}

fn valid_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 64
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

async fn small_json(mut response: reqwest::Response) -> Result<Value, String> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "memory control response read failed")?
    {
        if bytes.len() + chunk.len() > 64 * 1024 {
            return Err("memory control response exceeded 64 KiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "memory control response was not JSON".into())
}

/// Reserve memory only when a local legacy Laya endpoint explicitly advertises
/// protocol v1. Unsupported/off/cloud/unreachable endpoints are harmless skips.
/// Once support is advertised, a failed reserve is an actionable error.
pub async fn reserve(
    cfg: &Config,
    owner: &str,
    ttl_seconds: u64,
) -> Result<Option<MemoryLease>, String> {
    if !matches!(cfg.backend.as_str(), "local" | "auto") || cfg.local_runtime != "laya" {
        return Ok(None);
    }
    let Some(base) = local_base(&cfg.laya_url) else {
        return Ok(None);
    };
    if !valid_owner(owner) || !(1..=3600).contains(&ttl_seconds) {
        return Err("invalid memory reservation owner or TTL (must be 1..3600 seconds)".into());
    }
    let http = Client::builder()
        .timeout(CONTROL_TIMEOUT)
        .connect_timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| "could not construct local memory control client")?;
    let mut health_url = base.clone();
    health_url.set_path("/health");
    let health = match http.get(health_url).send().await {
        Ok(response) if response.status().is_success() => small_json(response).await.ok(),
        _ => None,
    };
    let supported = health.is_some_and(|health| {
        health["memory_management"]["version"].as_u64() == Some(1)
            && health["memory_management"]["reservations"].as_bool() == Some(true)
    });
    if !supported {
        return Ok(None);
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = format!(
        "kritic-{}-{stamp}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    );
    let mut lease = MemoryLease {
        request: Some(LeaseRequest {
            http,
            base,
            id,
            owner: owner.to_owned(),
            // Start before reserve I/O: unloading/acknowledgement also consumes TTL.
            work_deadline: tokio::time::Instant::now()
                + if ttl_seconds > 30 {
                    Duration::from_secs(ttl_seconds - 30)
                } else {
                    Duration::from_secs(ttl_seconds) / 2
                },
        }),
    };
    // Guard must exist across this await, including network-error and abort paths.
    let result = lease.acquire(ttl_seconds).await;
    if let Err(error) = result {
        let cleanup = lease.release().await;
        return Err(match cleanup {
            Ok(()) => error,
            Err(cleanup) => format!("{error}; {cleanup}"),
        });
    }
    Ok(Some(lease))
}

impl MemoryLease {
    async fn acquire(&self, ttl_seconds: u64) -> Result<(), String> {
        let request = self
            .request
            .as_ref()
            .ok_or("memory reservation is no longer armed")?;
        let mut url = request.base.clone();
        url.set_path("/v1/memory/reserve");
        let response = request.http.post(url)
            .json(&json!({"id": request.id, "owner": request.owner, "ttl_seconds": ttl_seconds}))
            .send().await
            .map_err(|_| "Kev advertised memory reservations but reserve failed; check the local Kev service before retrying")?;
        if !response.status().is_success() {
            return Err(format!(
                "Kev memory reservation rejected with HTTP {}; workload not started",
                response.status().as_u16()
            ));
        }
        let value = small_json(response).await?;
        if value["reserved"].as_bool() != Some(true)
            || value["id"].as_str() != Some(request.id.as_str())
        {
            return Err(
                "Kev memory reservation acknowledgement invalid; workload not started".into(),
            );
        }
        Ok(())
    }

    /// Explicit cleanup on completion/error. Failed release keeps the guard
    /// armed for one bounded Drop retry; expiry is the final crash safety net.
    pub async fn release(&mut self) -> Result<(), String> {
        let Some(request) = self.request.as_ref() else {
            return Ok(());
        };
        release_request(request).await?;
        self.request = None;
        Ok(())
    }
}

async fn release_request(request: &LeaseRequest) -> Result<(), String> {
    let mut url = request.base.clone();
    url.set_path("/v1/memory/release");
    let response = request.http.post(url)
        .json(&json!({"id": request.id, "owner": request.owner}))
        .send().await
        .map_err(|_| "Kev memory release failed; retry after the reservation TTL expires or check the local service")?;
    if !response.status().is_success() {
        return Err(format!(
            "Kev memory release rejected with HTTP {}; reservation will expire",
            response.status().as_u16()
        ));
    }
    let value = small_json(response).await?;
    if value["released"].as_bool().is_none() {
        return Err("Kev memory release acknowledgement invalid; reservation will expire".into());
    }
    Ok(())
}

impl Drop for MemoryLease {
    fn drop(&mut self) {
        let Some(request) = self.request.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                // Cancellation cannot await cleanup. Expiry bounds retention if
                // the runtime shuts down or this best-effort release fails.
                let _ = release_request(&request).await;
            });
        }
    }
}

/// Run local heavy work only after acknowledged yield, and release before the
/// caller can perform Kev triage. Error paths preserve both work/cleanup errors.
pub async fn with_reservation<T>(
    cfg: &Config,
    owner: &str,
    ttl_seconds: u64,
    work: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let mut lease = reserve(cfg, owner, ttl_seconds).await?;
    let result = if let Some(request) = lease.as_ref().and_then(|lease| lease.request.as_ref()) {
        if tokio::time::Instant::now() >= request.work_deadline {
            Err("Kev memory reservation deadline reached before work started; retry with a longer TTL".into())
        } else {
            tokio::time::timeout_at(request.work_deadline, work)
                .await
                .unwrap_or_else(|_| Err("Work exceeded the Kev memory reservation deadline; shorten the workload or increase its timeout within the reservation limit".into()))
        }
    } else {
        work.await
    };
    let cleanup = match lease.as_mut() {
        Some(lease) => lease.release().await,
        None => Ok(()),
    };
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; {cleanup}")),
    }
}

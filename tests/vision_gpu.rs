//! Kev on the vision GPU while the vision model is idle, with fake local HTTP services.
//!   cargo test --test vision_gpu

use kritic::config::Config;
use kritic::kev::KevClient;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A local HTTP server answering every GET with `body(path)`; None = 404.
async fn serve(body: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let body = Arc::new(body);
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let resp = match body(&path) {
                    Some(b) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
                        b.len()
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .into()
                    }
                };
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    url
}

fn laya() -> impl Fn(&str) -> Option<String> + Send + Sync + 'static {
    |p: &str| {
        (p == "/health")
            .then(|| r#"{"status":"ok","device":"vulkan:0","model":"kev-4b"}"#.to_string())
    }
}

fn ollama(loaded: bool) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static {
    move |p: &str| {
        (p == "/api/ps").then(|| {
            if loaded {
                r#"{"models":[{"name":"qwen2.5vl:7b"}]}"#
            } else {
                r#"{"models":[]}"#
            }
            .to_string()
        })
    }
}

/// A GPU router: no `/api/ps`, lanes on two GPUs; `busy` marks the GPU-aaaa lane.
fn router(busy: bool) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static {
    move |p: &str| {
        (p == "/v1/lanes").then(|| {
            let job = if busy { r#""job-1""# } else { "null" };
            format!(
                r#"[{{"id":"heavy","gpu":{{"uuid":"GPU-aaaa"}},"busy":{job},"resident_model":null}},
                   {{"id":"small","gpu":{{"uuid":"GPU-bbbb"}},"busy":null,"resident_model":"embed"}}]"#
            )
        })
    }
}

fn cfg(primary: &str, vision: &str, alt: Option<&str>) -> Config {
    let mut c = Config::load();
    c.laya_url = primary.into();
    c.ollama_url = vision.into();
    c.kev_cpu_only = false;
    c.vision_gpu_kev_url = alt.map(String::from);
    c.vision_gpu_kev_start = None;
    c.vision_gpu_kev_stop = None;
    c.vision_gpu_kev_uuid = Some("GPU-aaaa".into());
    c.vision_gpu_kev_idle_s = 0;
    c
}

#[test]
fn ollama_ps_idle() {
    use serde_json::json;
    assert!(kritic::gpu::ps_idle(&json!({"models": []})));
    assert!(kritic::gpu::ps_idle(&json!({})));
    assert!(!kritic::gpu::ps_idle(&json!({"models": [{"name": "m"}]})));
}

#[test]
fn router_lanes_idle_on_the_kev_gpu_only() {
    use serde_json::json;
    let v = json!([
        {"gpu": {"uuid": "GPU-aaaa"}, "busy": null, "resident_model": null},
        {"gpu": {"uuid": "GPU-bbbb"}, "busy": null, "resident_model": "m"}
    ]);
    assert_eq!(kritic::gpu::lanes_idle(&v, Some("gpu-AAAA")), Some(true));
    assert_eq!(
        kritic::gpu::lanes_idle(&v, None),
        Some(false),
        "unscoped: every lane counts"
    );
    assert_eq!(kritic::gpu::lanes_idle(&json!({"x": 1}), None), None);
}

#[tokio::test]
async fn kev_follows_a_gpu_router_lane() {
    let primary = serve(laya()).await;
    let alt = serve(laya()).await;
    let idle = serve(router(false)).await;
    let busy = serve(router(true)).await;
    let k = KevClient::new(Arc::new(cfg(&primary, &idle, Some(&alt))));
    assert_eq!(
        k.endpoint().await,
        alt,
        "router lane idle (the other lane's model ignored)"
    );
    let k = KevClient::new(Arc::new(cfg(&primary, &busy, Some(&alt))));
    assert_eq!(k.endpoint().await, primary, "router lane busy");
}

#[tokio::test]
async fn kev_uses_the_vision_gpu_only_while_vision_is_idle() {
    let primary = serve(laya()).await;
    let alt = serve(laya()).await;
    let idle = serve(ollama(false)).await;
    let busy = serve(ollama(true)).await;

    let k = KevClient::new(Arc::new(cfg(&primary, &idle, None)));
    assert_eq!(k.endpoint().await, primary, "unset: always the primary");

    let k = KevClient::new(Arc::new(cfg(&primary, &idle, Some(&alt))));
    assert_eq!(k.endpoint().await, alt, "vision idle: the vision-GPU Kev");

    let k = KevClient::new(Arc::new(cfg(&primary, &busy, Some(&alt))));
    assert_eq!(k.endpoint().await, primary, "vision loaded: the primary");

    let k = KevClient::new(Arc::new(cfg(&primary, "http://127.0.0.1:1", Some(&alt))));
    assert_eq!(k.endpoint().await, alt, "Ollama not running counts as idle");

    let k = KevClient::new(Arc::new(cfg(&primary, &idle, Some("http://127.0.0.1:1"))));
    assert_eq!(
        k.endpoint().await,
        primary,
        "unreachable and no start command: the primary"
    );
}

#[tokio::test]
async fn hidden_commands_report_failure_and_free_is_a_no_op_without_config() {
    assert!(
        kritic::gpu::run_hidden("exit 0", std::time::Duration::from_secs(20))
            .await
            .is_ok()
    );
    let e = kritic::gpu::run_hidden("exit 3", std::time::Duration::from_secs(20))
        .await
        .unwrap_err();
    assert!(e.contains("exited with"), "{e}");
    let primary = serve(laya()).await;
    assert!(!kritic::gpu::free_vision_gpu(&cfg(&primary, "http://127.0.0.1:1", None)).await);
}

/// KRITIC_LIVE=1 with `kev.vision_gpu_*` configured: a real decision goes to the vision-GPU Kev
/// while the vision model is idle, and `free_vision_gpu` stops it again.
#[tokio::test]
async fn live_vision_gpu_round_trip() {
    if std::env::var("KRITIC_LIVE").is_err() {
        return;
    }
    use kritic::kev::Decider;
    let cfg = Config::load();
    let alt = cfg
        .vision_gpu_kev_url
        .clone()
        .expect("kev.vision_gpu_laya_url configured");
    let k = KevClient::new(Arc::new(cfg.clone()));
    let q = kritic::kev::Question {
        kind: "choice",
        instructions: "Pick one.".into(),
        criteria: Some(serde_json::json!({"0": "value 0", "1": "value 1"})),
    };
    let qs = std::collections::BTreeMap::from([("a".to_string(), q)]);
    let d = k
        .decide(&serde_json::json!({"x": 1}), &qs)
        .await
        .expect("decide");
    eprintln!("endpoint {} latency {} ms", k.last_endpoint(), d.latency_ms);
    assert_eq!(k.last_endpoint(), alt);
    assert!(kritic::gpu::free_vision_gpu(&cfg).await, "stopped");
    assert!(
        reqwest::get(format!("{alt}/health")).await.is_err(),
        "down after free"
    );
}

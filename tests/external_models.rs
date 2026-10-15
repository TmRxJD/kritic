//! Loopback lifecycle tests: never launch/kill a process, download a model or contact a GPU.
use kritic::{config::Config, gpu, kev::KevClient};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    responses: Vec<(u16, Value)>,
) -> (String, tokio::task::JoinHandle<Vec<(String, Value)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = vec![];
        for (status, value) in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![];
            let mut buffer = [0u8; 1024];
            let offset = loop {
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
                assert!(bytes.len() < 65536);
                if let Some(offset) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    break offset + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..offset]).to_string();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while bytes.len() < offset + length {
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
                assert!(bytes.len() < 65536);
            }
            requests.push((
                headers.lines().next().unwrap().into(),
                if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[offset..offset + length]).unwrap()
                },
            ));
            let body = value.to_string();
            socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    (url, task)
}

fn config(directory: &std::path::Path) -> Config {
    let mut cfg = Config::load();
    cfg.config_errors.clear();
    cfg.backend = "local".into();
    cfg.vision_backend = "local".into();
    cfg.kev_externally_managed = true;
    cfg.vision_externally_managed = true;
    cfg.kev_cpu_only = false;
    cfg.laya_exe = directory.join("must-not-start-laya");
    cfg.ollama_exe = directory.join("must-not-start-ollama");
    cfg.log_path = directory.join("calls.jsonl");
    cfg.runs_dir = directory.join("runs");
    cfg
}

#[tokio::test]
async fn unavailable_external_services_return_owner_errors_without_launching() {
    let directory = tempfile::tempdir().unwrap();
    let mut cfg = config(directory.path());
    let (url, request) = server(vec![(503, json!({"error":"offline"}))]).await;
    cfg.laya_url = url;
    let error = KevClient::new(Arc::new(cfg.clone()))
        .ensure_up()
        .await
        .unwrap_err();
    assert!(
        error.contains("externally managed laya unreachable"),
        "{error}"
    );
    assert_eq!(request.await.unwrap().len(), 1);
    let (url, request) = server(vec![(503, json!({"error":"offline"}))]).await;
    cfg.ollama_url = url;
    let error = kritic::vision::ensure_ollama(&reqwest::Client::new(), &cfg)
        .await
        .unwrap_err();
    assert!(
        error.contains("externally managed vision service unreachable"),
        "{error}"
    );
    assert_eq!(request.await.unwrap().len(), 1);
    assert!(!directory.path().join("laya.log").exists());
}

#[tokio::test]
async fn external_endpoints_allow_inference_without_eviction_or_keep_alive_override() {
    let directory = tempfile::tempdir().unwrap();
    let mut cfg = config(directory.path());
    let (url, health) = server(vec![(200, json!({"status":"ok"}))]).await;
    cfg.laya_url = url;
    KevClient::new(Arc::new(cfg.clone()))
        .ensure_up()
        .await
        .unwrap();
    assert_eq!(health.await.unwrap().len(), 1);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    cfg.ollama_url = format!("http://{}", listener.local_addr().unwrap());
    assert!(!gpu::stop_laya(&cfg).await);
    gpu::unload_vision(&cfg).await;
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(gpu::requires_gpu(false, gpu::Workload::Kev));
    assert!(gpu::requires_gpu(false, gpu::Workload::Vision));
    let (url, requests) = server(vec![
        (200, json!({"version":"fixture"})),
        (200, json!({"message":{"content":"{\"mark\":1}"}})),
    ])
    .await;
    cfg.ollama_url = url;
    let image = directory.path().join("fixture.png");
    std::fs::write(&image, b"fixture").unwrap();
    assert_eq!(
        kritic::vision::pick(&cfg, &image, "Open", &[], &["Open".into()], 1)
            .await
            .unwrap(),
        0
    );
    let requests = requests.await.unwrap();
    assert!(requests[1].0.starts_with("POST /api/chat"));
    assert!(requests[1].1.get("keep_alive").is_none());
    assert_eq!(requests[1].1["model"], cfg.vision_model);
}

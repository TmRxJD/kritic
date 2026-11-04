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
    raw_server(
        responses
            .into_iter()
            .map(|(status, value)| (status, "application/json", value.to_string()))
            .collect(),
    )
    .await
}

async fn raw_server(
    responses: Vec<(u16, &'static str, String)>,
) -> (String, tokio::task::JoinHandle<Vec<(String, Value)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = vec![];
        for (status, content_type, body) in responses {
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
            socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    (url, task)
}

#[tokio::test]
async fn vision_preserves_http_rejections_and_explains_capture_limits() {
    let directory = tempfile::tempdir().unwrap();
    let image = directory.path().join("fixture.png");
    // This test exercises HTTP handling. The loopback peer never decodes the image.
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nfixture").unwrap();
    let mut cfg = config(directory.path());
    let cases = [
        (413, "text/plain", "Failed to buffer the request body: length limit exceeded".into(),
            "Ollama HTTP 413", "smaller JPEG attachments"),
        (503, "application/json", json!({"error":"request (5797 tokens) exceeds the available context size (4096 tokens)"}).to_string(),
            "Ollama HTTP 503", "lower-resolution images"),
        (200, "text/plain", "invalid success body".into(), "Ollama returned non-JSON", "error decoding response body"),
    ];
    for (status, content_type, body, headline, detail) in cases {
        let (url, requests) = raw_server(vec![
            (
                200,
                "application/json",
                json!({"version":"fixture"}).to_string(),
            ),
            (status, content_type, body),
        ])
        .await;
        cfg.ollama_url = url;
        let result = kritic::vision::critique(&cfg, &image, "fixture").await;
        assert!(!result.model_ok);
        let error = result.error.unwrap();
        assert!(
            error.contains(headline) && error.contains(detail),
            "{error}"
        );
        if status == 413 {
            assert!(error.contains("length limit exceeded"), "{error}");
        }
        if status == 503 {
            assert!(error.contains("5797 tokens"), "{error}");
        }
        let requests = requests.await.unwrap();
        assert!(requests[1].0.starts_with("POST /api/chat"));
    }
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
async fn scene_review_requires_grounded_partial_evidence_and_omits_ui_scores() {
    use kritic::visual_quality::{QualityTarget, ReviewKind, parse_quality_for};
    let dimension = json!({"score":7,"coverage":"partial",
        "evidence":[{"element":"shoreline bend","observation":"The subject bend is left of the road; reference_1 shows it behind the road."}],
        "improvement":"Align the subject bend with the road."});
    let valid = json!({"craft":{
        "viewpoint":dimension,"terrain_and_shoreline":dimension,
        "landmark_layout":dimension,"vegetation_and_surfaces":dimension
    }});
    let mut empty = valid.clone();
    empty["craft"]["viewpoint"]["evidence"] = json!([]);
    assert!(
        parse_quality_for(
            &empty,
            QualityTarget::Showcase,
            9.0,
            ReviewKind::SceneReference
        )
        .unwrap_err()
        .contains("requires one observation")
    );

    let directory = tempfile::tempdir().unwrap();
    let subject = directory.path().join("render.png");
    let reference = directory.path().join("photo.png");
    std::fs::write(&subject, b"\x89PNG\r\n\x1a\nsubject").unwrap();
    std::fs::write(&reference, b"\x89PNG\r\n\x1a\nreference").unwrap();
    let mut cfg = config(directory.path());
    let (url, requests) = server(vec![
        (200, json!({"version":"fixture"})),
        (200, json!({"message":{"content":valid.to_string()}})),
    ])
    .await;
    cfg.ollama_url = url;
    let result = kritic::vision::critique_with_kind(
        &cfg,
        &subject,
        "Compare shorelines",
        QualityTarget::Showcase,
        9.0,
        &[reference],
        ReviewKind::SceneReference,
    )
    .await;
    assert!(result.model_ok, "{:?}", result.error);
    assert!(result.scores.is_none() && result.approval.is_none());
    assert_eq!(
        result.quality_assessment.unwrap().rubric_version,
        "scene-reference-v1"
    );
    let requests = requests.await.unwrap();
    let body = &requests[1].1;
    assert!(body["format"]["properties"].get("layout_clarity").is_none());
    assert_eq!(body["messages"][1]["images"].as_array().unwrap().len(), 2);
    assert!(
        body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("terrain_and_shoreline")
    );
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

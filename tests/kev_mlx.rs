//! Native MLX contract tests; loopback fixtures never load weights.
use kritic::{
    config::Config,
    kev::{Decider, KevClient, Question},
    kev_native,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
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
            let redirect = if status == 302 {
                "Location: http://127.0.0.1:9/\r\n"
            } else {
                ""
            };
            socket.write_all(format!("HTTP/1.1 {status} Test\r\n{redirect}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    (url, task)
}

fn config() -> Config {
    let mut cfg = Config::load();
    cfg.config_errors.clear();
    cfg.backend = "local".into();
    cfg.local_runtime = "mlx".into();
    cfg.kev_externally_managed = false;
    cfg.model = "kev-latest".into();
    cfg.kev_request_s = 1;
    cfg
}
fn card(cfg: &Config) -> Value {
    json!({"models":[{"name":cfg.model,"run":cfg.native_run,"base":cfg.native_base,"device":"mps","backend":"mlx","dtype":cfg.native_dtype}]})
}
fn questions() -> BTreeMap<String, Question> {
    [
        (
            "n".into(),
            Question {
                kind: "noul",
                instructions: "Is the sky blue?".into(),
                criteria: None,
            },
        ),
        (
            "c".into(),
            Question {
                kind: "choice",
                instructions: "Pick a color".into(),
                criteria: Some(json!({"blue":"","red":""})),
            },
        ),
        (
            "s".into(),
            Question {
                kind: "score",
                instructions: "Rate".into(),
                criteria: Some(json!(["low", "high"])),
            },
        ),
    ]
    .into()
}
fn response() -> Value {
    json!({"model":"kev-latest","answers":{
    "n":{"type":"noul","noul":0.8},
    "c":{"type":"choice","choice":"blue","confidence":0.6,"probabilities":{"blue":0.8,"red":0.2}},
    "s":{"type":"score","score":0.8,"confidence":0.6,"probabilities":{"0":0.2,"1":0.8},"legend":{"0":"low","1":"high"}}
},"usage":{"input_tokens":75,"output_tokens":60}})
}
#[tokio::test]
async fn upstream_success_uses_models_and_systemone_and_is_advisory() {
    let mut cfg = config();
    let (url, requests) = server(vec![(200, card(&cfg)), (200, response())]).await;
    cfg.native_url = url;
    let client = KevClient::new(Arc::new(cfg));
    let d = client
        .decide(&json!("The sky is blue."), &questions())
        .await
        .unwrap();
    assert_eq!(d.answers.len(), 3);
    assert_eq!(d.input_tokens, 75);
    assert!(!d.truncated);
    assert_eq!(d.answers["n"].confidence, 0.8);
    assert!(
        client
            .model_identity()
            .unwrap()
            .starts_with("unverified-mlx:")
    );
    let requests = requests.await.unwrap();
    assert_eq!(requests[0].0, "GET /v1/models HTTP/1.1");
    assert_eq!(requests[1].0, "POST /v1/systemone HTTP/1.1");
    assert_eq!(
        requests[1].1["questions"]["c"]["criteria"],
        json!({"blue":"","red":""})
    );
}
#[test]
fn rejects_wrong_model_backend_precision_base_run_and_cpu() {
    let cfg = config();
    for key in ["run", "base", "backend", "device", "dtype", "name"] {
        let mut c = card(&cfg);
        c["models"][0][key] = json!("wrong");
        assert!(
            serde_json::from_value::<kev_native::Models>(c)
                .unwrap()
                .validate(&cfg)
                .is_err(),
            "{key}"
        );
    }
    assert!(
        serde_json::from_value::<kev_native::Models>(json!({"models":[]}))
            .unwrap()
            .validate(&cfg)
            .is_err()
    );
}
#[test]
fn malformed_answers_never_default_and_truncation_is_explicit() {
    let mut cases = vec![];
    let mut r = response();
    r["answers"].as_object_mut().unwrap().remove("n");
    cases.push(r);
    let mut r = response();
    r["answers"]["extra"] = json!({"type":"noul","noul":0.5});
    cases.push(r);
    let mut r = response();
    r["answers"]["n"]["noul"] = json!(1.1);
    cases.push(r);
    let mut r = response();
    r["answers"]["n"]["type"] = json!("score");
    cases.push(r);
    let mut r = response();
    r["answers"]["c"]["confidence"] = json!(-1);
    cases.push(r);
    let mut r = response();
    r["answers"]["c"]["probabilities"] = json!({"blue":0.8,"other":0.2});
    cases.push(r);
    let mut r = response();
    r["answers"]["s"]["probabilities"] = json!({"0":0.2,"3":0.8});
    cases.push(r);
    let mut r = response();
    r["answers"]["s"]["legend"] = json!({"0":"high", "1":"low"});
    cases.push(r);
    let mut r = response();
    r["usage"] = json!({});
    cases.push(r);
    let mut r = response();
    r["model"] = json!("other");
    cases.push(r);
    for r in cases {
        assert!(kev_native::normalize(r, &questions(), "kev-latest").is_err());
    }
    let mut r = response();
    r["truncated"] = json!(true);
    assert_eq!(
        kev_native::normalize(r, &questions(), "kev-latest").unwrap()["truncated"],
        true
    );
}
#[tokio::test]
async fn rejects_http_errors_and_mismatched_readiness_without_starting_laya() {
    for status in [302, 401, 422, 500] {
        let mut cfg = config();
        let (url, task) = server(vec![
            (200, card(&cfg)),
            (status, json!({"detail":"fixture failure"})),
        ])
        .await;
        cfg.native_url = url;
        let e = KevClient::new(Arc::new(cfg))
            .decide(&json!("short"), &questions())
            .await
            .unwrap_err();
        assert!(e.contains(&status.to_string()), "{e}");
        task.await.unwrap();
    }
    let mut cfg = config();
    let mut c = card(&cfg);
    c["models"][0]["backend"] = json!("torch");
    let (url, task) = server(vec![(200, c)]).await;
    cfg.native_url = url;
    assert!(
        KevClient::new(Arc::new(cfg.clone()))
            .ensure_up()
            .await
            .unwrap_err()
            .contains("identity mismatch")
    );
    task.await.unwrap();
    assert!(!kritic::gpu::stop_laya(&cfg).await);
}
#[tokio::test]
async fn timeout_is_bounded_and_external_service_survives() {
    let mut cfg = config();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    cfg.native_url = format!("http://{}", listener.local_addr().unwrap());
    let c = card(&cfg).to_string();
    let task = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut b = [0; 4096];
        assert!(s.read(&mut b).await.unwrap() > 0);
        s.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{c}",
                c.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        drop(s);
        let (_s, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    });
    let started = std::time::Instant::now();
    let e = KevClient::new(Arc::new(cfg))
        .decide(&json!("short"), &questions())
        .await
        .unwrap_err();
    assert!(e.contains("timed out") || e.contains("stalled"), "{e}");
    assert!(started.elapsed().as_secs() < 3);
    assert!(!task.is_finished());
    task.abort();
}
#[tokio::test]
async fn short_context_overflow_is_refused_before_inference() {
    let mut cfg = config();
    cfg.max_seq_tokens = 10;
    let (url, task) = server(vec![(200, card(&cfg))]).await;
    cfg.native_url = url;
    assert!(
        KevClient::new(Arc::new(cfg))
            .decide(&json!("short"), &questions())
            .await
            .unwrap_err()
            .contains("short-context")
    );
    assert_eq!(task.await.unwrap().len(), 1);
}
#[tokio::test]
#[ignore = "requires explicitly started pinned upstream 0.8B MLX loopback service"]
async fn live_mlx_0_8b() {
    let cfg = Config::load();
    assert!(cfg.config_errors.is_empty(), "{:?}", cfg.config_errors);
    assert_eq!(cfg.local_runtime, "mlx");
    assert!(cfg.native_run.starts_with("jaredpalmer/kev-0.8b@"));
    let client = KevClient::new(Arc::new(cfg));
    for i in 0..4 {
        let t = std::time::Instant::now();
        let d = client
            .decide(&json!("The sky is blue."), &questions())
            .await
            .unwrap();
        assert_eq!(d.answers.len(), 3);
        assert!(!d.truncated);
        println!(
            "live request {i}: elapsed_ms={} decision={d:?}",
            t.elapsed().as_millis()
        );
    }
    assert!(
        client
            .model_identity()
            .unwrap()
            .starts_with("unverified-mlx:")
    );
}

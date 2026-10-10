//! Real HTTP envelopes against loopback mocks; no paid requests or model downloads.
use kritic::config::Config;
use kritic::kev::{Backend, Decider, KevClient, Question};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn mock(status: &str, body: &str) -> (String, tokio::task::JoinHandle<(String, Value)>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0; 4096];
        let (headers, offset) = loop {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = buf.windows(4).position(|v| v == b"\r\n\r\n") {
                break (String::from_utf8_lossy(&buf[..i]).to_string(), i + 4);
            }
        };
        let len: usize = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap();
        while buf.len() < offset + len {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            buf.extend_from_slice(&chunk[..n]);
        }
        let value = serde_json::from_slice(&buf[offset..offset + len]).unwrap();
        socket.write_all(response.as_bytes()).await.unwrap();
        (headers, value)
    });
    (url, task)
}

fn config(dir: &std::path::Path, provider: &str, url: String) -> Config {
    let mut cfg = Config::load();
    cfg.config_errors.clear();
    cfg.backend = "cloud".into();
    cfg.cloud_provider = provider.into();
    cfg.cloud_model = "kev-4b".into();
    cfg.cloud_url = if provider == "gemini" {
        format!("{url}/models/kev-4b:generateContent")
    } else {
        url
    };
    cfg.cloud_key_env = "KRITIC_TEST_UNUSED_KEY".into();
    cfg.cloud_key_file = dir.join("key");
    cfg.cloud_key_file_explicit = true;
    std::fs::write(&cfg.cloud_key_file, "test-secret").unwrap();
    cfg.log_path = dir.join("calls.jsonl");
    cfg.runs_dir = dir.join("runs");
    cfg.kev_request_s = 3;
    cfg
}

fn questions() -> BTreeMap<String, Question> {
    BTreeMap::from([(
        "ok".into(),
        Question {
            kind: "noul",
            instructions: "Does the code implement the task?".into(),
            criteria: Some(json!({"true":"yes","false":"no"})),
        },
    )])
}

#[tokio::test]
async fn every_cloud_transport_validates_and_is_advisory() {
    let dir = tempfile::tempdir().unwrap();
    let answer = json!({"answers":{"ok":{"type":"noul","noul":0.9,"confidence":0.9}}}).to_string();
    let fixtures = [
        (
            "openai-compatible",
            json!({"choices":[{"finish_reason":"stop","message":{"content":answer}}],"usage":{"prompt_tokens":12,"completion_tokens":7}}),
        ),
        (
            "openai",
            json!({"choices":[{"finish_reason":"stop","message":{"content":answer}}],"usage":{"prompt_tokens":12,"completion_tokens":7}}),
        ),
        (
            "anthropic",
            json!({"stop_reason":"end_turn","content":[{"type":"text","text":answer}],"usage":{"input_tokens":12,"output_tokens":7}}),
        ),
        (
            "gemini",
            json!({"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":answer}]}}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":7}}),
        ),
    ];
    for (provider, response) in fixtures {
        let (url, seen) = mock("200 OK", &response.to_string()).await;
        let cfg = config(dir.path(), provider, url);
        let identity = cfg.decision_identity();
        assert!(!kritic::spec::get("code_chunk").gates_apply_for(&identity));
        let client = KevClient::new(Arc::new(cfg));
        let d = client
            .decide(&json!({"code":"fn ok() {}"}), &questions())
            .await
            .unwrap();
        assert_eq!(d.answers["ok"].value, 0.9);
        assert_eq!(d.input_tokens, 12);
        let (headers, body) = seen.await.unwrap();
        assert!(headers.contains("test-secret"));
        assert!(body.to_string().contains("fn ok()"));
        assert!(
            !format!(
                "{:?}",
                Backend::Cloud {
                    key: "test-secret".into()
                }
            )
            .contains("test-secret")
        );
    }
}

#[tokio::test]
async fn refuses_bad_answers_truncation_and_secret_echo() {
    let dir = tempfile::tempdir().unwrap();
    for answer in [
        json!({"answers":{}}),
        json!({"answers":{"ok":{"type":"noul","noul":1,"confidence":3}}}),
        json!({"answers":{"ok":{"type":"noul","noul":0.9,"confidence":0.9},"extra":{}}}),
    ] {
        let response =
            json!({"choices":[{"finish_reason":"stop","message":{"content":answer.to_string()}}]});
        let (url, seen) = mock("200 OK", &response.to_string()).await;
        let client = KevClient::new(Arc::new(config(dir.path(), "openai-compatible", url)));
        assert!(client.decide(&json!({}), &questions()).await.is_err());
        seen.await.unwrap();
    }
    let raw = json!({"choices":[{"finish_reason":"length","message":{"content":"{}"}}]});
    assert!(kritic::provider::decode("openai-compatible", &raw).is_err());
    let (url, seen) = mock(
        "401 Unauthorized",
        "{\"error\":\"test-secret private source\"}",
    )
    .await;
    let cfg = config(dir.path(), "openai-compatible", url);
    let err = kritic::provider::exchange(&cfg, "test-secret", &json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("401"));
    assert!(!err.contains("test-secret"));
    seen.await.unwrap();
}

#[test]
fn rejects_unsafe_endpoints_and_invalid_probabilities() {
    let dir = tempfile::tempdir().unwrap();
    for url in [
        "http://example.com/api",
        "https://key@example.com/api",
        "https://example.com/api?key=secret",
        "https://example.com/api#secret",
    ] {
        assert!(
            kritic::provider::endpoint(&config(dir.path(), "openai-compatible", url.into()))
                .is_err()
        );
    }
    let q = Question {
        kind: "choice",
        instructions: "pick".into(),
        criteria: Some(json!({"a":"first","b":"second"})),
    };
    for p in [
        json!({"a":0.5}),
        json!({"a":1.5,"b":-0.5}),
        json!({"a":0.3,"b":0.3}),
        json!({"a":0.5,"b":"x"}),
    ] {
        assert!(
            kritic::kev::parse_answer(
                "pick",
                &q,
                &json!({"type":"choice","choice":"a","probabilities":p,"confidence":0.9})
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn off_status_does_not_touch_the_model_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let (url, seen) = mock("200 OK", "{}").await;
    let mut cfg = config(dir.path(), "openai-compatible", url.clone());
    cfg.backend = "off".into();
    cfg.laya_url = url;
    let client = KevClient::new(Arc::new(cfg));
    let status = kritic::tools::status(&client, false).await;
    assert!(!status.gates_calibrated);
    assert!(status.enabled_tools.iter().any(|n| n == "contract_critic"));
    assert!(!status.enabled_tools.iter().any(|n| n == "critic_score"));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), seen)
            .await
            .is_err()
    );
}

#[test]
fn image_envelopes_follow_each_provider() {
    let dir = tempfile::tempdir().unwrap();
    for provider in ["openai-compatible", "anthropic", "gemini"] {
        let mut cfg = config(dir.path(), provider, "http://localhost/api".into());
        cfg.cloud_model = "vision-model".into();
        let request = kritic::provider::request(
            &cfg,
            "vision-model",
            "JSON only",
            "review",
            Some(("image/png", "iVBORdata")),
        )
        .unwrap();
        assert!(request.to_string().contains("iVBORdata"));
        assert!(request.to_string().contains("image/png"));
    }
}

#[test]
fn default_saved_credentials_do_not_follow_provider_or_origin_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(
        dir.path(),
        "openai-compatible",
        "http://localhost:1234/api".into(),
    );
    cfg.cloud_key_file_explicit = false;
    cfg.save_cloud_key("test-secret").unwrap();
    assert_eq!(cfg.cloud_key().as_deref(), Some("test-secret"));
    cfg.cloud_url = "http://localhost:4321/api".into();
    assert!(cfg.cloud_key().is_none());
    cfg.cloud_url = "http://localhost:1234/api".into();
    cfg.cloud_provider = "anthropic".into();
    assert!(cfg.cloud_key().is_none());
}

#[test]
fn gemini_rejects_false_model_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), "gemini", "http://localhost/api".into());
    assert!(kritic::provider::endpoint(&cfg).is_ok());
    assert!(kritic::provider::request(&cfg, "other-model", "s", "u", None).is_err());
    cfg.cloud_model = "other-model".into();
    assert!(kritic::provider::endpoint(&cfg).is_err());
}

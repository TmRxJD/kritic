//! Cloud Jev backend against a local mock of the evaluate API: request translation, answer
//! translation, key handling, and gates turning advisory under an uncalibrated model.
//! One test function: it sets the process environment.

use std::collections::BTreeMap;
use std::sync::Arc;

use kritic::config::Config;
use kritic::kev::{Backend, Decider, KevClient, Question, cloud_answer, cloud_question};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Serve one canned JSON response per connection and record each request body.
async fn mock(status: u16, body: Value) -> (String, Arc<tokio::sync::Mutex<Vec<(String, Value)>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/evaluate", listener.local_addr().unwrap());
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            // read headers, then Content-Length bytes of body
            let (head, body_start) = loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break (String::from_utf8_lossy(&buf[..i]).to_string(), i + 4);
                }
            };
            let len: usize = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                .unwrap_or(0);
            while buf.len() < body_start + len {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
            }
            let auth = head.lines().find_map(|l| l.strip_prefix("authorization: ").or_else(|| l.strip_prefix("Authorization: "))).unwrap_or("").to_string();
            let req: Value = serde_json::from_slice(&buf[body_start..body_start + len]).unwrap();
            seen2.lock().await.push((auth, req));
            let text = body.to_string();
            let resp = format!("HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len());
            sock.write_all(resp.as_bytes()).await.unwrap();
        }
    });
    (url, seen)
}

fn q(kind: &'static str, criteria: Value) -> Question {
    Question { kind, instructions: format!("a {kind} question about `code`"), criteria: Some(criteria) }
}

#[test]
fn translations_are_exact() {
    let yn = q("noul", json!({"true": "yes", "false": "no"}));
    assert_eq!(cloud_question(&yn)["type"], "boolean");
    assert_eq!(cloud_question(&yn)["criteria"]["true"], "yes");

    let a = cloud_answer(&yn, &json!({"type": "boolean", "probability": 0.2}));
    assert_eq!(a, json!({"type": "noul", "noul": 0.2, "confidence": 0.8}));

    let ch = q("choice", json!({"a": "first", "b": "second"}));
    let a = cloud_answer(&ch, &json!({"type": "choice", "choice": "b", "probabilities": {"a": 0.3, "b": 0.7}}));
    assert_eq!(a["confidence"], 0.7);

    // Score: rung probabilities keyed by the rung text give the expected 0-based index.
    let sc = q("score", json!(["low: bad", "mid: ok", "high: good"]));
    let a = cloud_answer(&sc, &json!({"type": "score", "score": 3, "probabilities": {"low: bad": 0.0, "mid: ok": 0.5, "high: good": 0.5}}));
    assert_eq!(a["score"], 1.5);
    // Without usable probabilities a 1-based top score is shifted to the last rung.
    let a = cloud_answer(&sc, &json!({"type": "score", "score": 3}));
    assert_eq!(a["score"], 2.0);
}

#[tokio::test]
async fn cloud_backend_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary that touches the environment.
    unsafe {
        for (_, env) in kritic::config::KEYS {
            std::env::remove_var(env);
        }
        std::env::remove_var("AI_GATEWAY_API_KEY");
        std::env::remove_var("KRITIC_CONFIG");
        std::env::set_var("KRITIC_HOME", home.path());
        // A dead port and no binary, so a laya running on the test machine isn't found.
        std::env::set_var("KRITIC_LAYA_URL", "http://127.0.0.1:9");
        std::env::set_var("KRITIC_LAYA_EXE", home.path().join("no-laya"));
    }

    // auto with nothing installed and no key: a clear error naming what to do.
    let k = KevClient::new(Arc::new(Config::load()));
    let err = k.resolve_backend().await.unwrap_err();
    assert!(err.contains("kritic setup"), "{err}");

    // off: never asks anything.
    unsafe { std::env::set_var("KRITIC_KEV_BACKEND", "off") };
    let k = KevClient::new(Arc::new(Config::load()));
    assert!(k.resolve_backend().await.unwrap_err().contains("off"));

    // cloud with a key file: requests are translated, answers validated.
    let (url, seen) = mock(200, json!({"model": "typesafe-ai/jev", "answers": {"ok": {"type": "boolean", "probability": 0.9}}, "usage": {"inputTokens": 42, "outputTokens": 1}})).await;
    std::fs::write(home.path().join("ai-gateway.key"), "test-key\n").unwrap();
    unsafe {
        std::env::set_var("KRITIC_KEV_BACKEND", "cloud");
        std::env::set_var("KRITIC_CLOUD_URL", &url);
    }
    let k = KevClient::new(Arc::new(Config::load()));
    assert_eq!(k.resolve_backend().await.unwrap(), Backend::Cloud { key: "test-key".into() });
    let mut questions = BTreeMap::new();
    questions.insert("ok".to_string(), q("noul", json!({"true": "fine", "false": "broken"})));
    let d = k.decide(&json!({"code": "x = 1"}), &questions).await.unwrap();
    assert_eq!(d.answers["ok"].value, 0.9);
    assert_eq!(d.input_tokens, 42);
    let (auth, req) = seen.lock().await[0].clone();
    assert_eq!(auth, "Bearer test-key");
    assert_eq!(req["model"], "typesafe-ai/jev");
    assert_eq!(req["questions"]["ok"]["type"], "boolean");

    // Built-in gates were calibrated on kev-4b: under Jev they no longer decide.
    let spec = &kritic::spec::builtin()["code_chunk"];
    assert!(!spec.gates_apply());
    assert!(spec.uncalibrated_note().unwrap().contains("typesafe-ai/jev"));

    // A rejected key is reported as such, not as a generic failure.
    let (url, _) = mock(403, json!({"error": {"message": "forbidden"}})).await;
    unsafe { std::env::set_var("KRITIC_CLOUD_URL", &url) };
    let k = KevClient::new(Arc::new(Config::load()));
    let err = k.decide(&json!({"code": "x"}), &questions).await.unwrap_err();
    assert!(err.contains("403") && err.contains("key"), "{err}");

    // An answer that is out of range is an error, never a default.
    let (url, _) = mock(200, json!({"answers": {"ok": {"type": "boolean", "probability": 1.7}}})).await;
    unsafe { std::env::set_var("KRITIC_CLOUD_URL", &url) };
    let k = KevClient::new(Arc::new(Config::load()));
    assert!(k.decide(&json!({"code": "x"}), &questions).await.unwrap_err().contains("out of"));
}

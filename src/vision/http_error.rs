//! Bounded local-model HTTP error decoding and actionable capture limits.

use serde_json::Value;

/// Proxies and request-limit middleware can reject before the Ollama JSON handler runs.
pub(super) async fn local_http_error(mut response: reqwest::Response) -> String {
    const MAX_BODY_BYTES: usize = 4096;
    const MAX_DETAIL_CHARS: usize = 320;
    let status = response.status();
    let mut bytes = Vec::new();
    while bytes.len() < MAX_BODY_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_BODY_BYTES - bytes.len();
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    let detail = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => value["error"].as_str().unwrap_or("").to_owned(),
        Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
    };
    let context_exceeded = detail.contains("exceed_context_size_error")
        || detail.contains("exceeds the available context size");
    let detail: String = detail
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DETAIL_CHARS)
        .collect();
    let mut error = format!("Ollama HTTP {status}");
    if !detail.is_empty() {
        error.push_str(": ");
        error.push_str(&detail);
    }
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        error.push_str(
            ". The encoded images exceed the endpoint request limit. Retry with smaller JPEG attachments or fewer reference images.",
        );
    } else if context_exceeded {
        error.push_str(". The vision context is full. Retry with lower-resolution images, fewer references, or shorter review context.");
    }
    error
}

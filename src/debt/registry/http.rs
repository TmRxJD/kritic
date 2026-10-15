//! Fixed-authority, credential-free metadata reads; registry links are never followed.
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;

pub(super) const MAX_RESPONSE: usize = 2 * 1024 * 1024;

pub(super) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .user_agent("kritic-public-metadata/0.1 (read-only dependency review)")
        .build()
        .map_err(|error| error.to_string())
}

pub(super) fn allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(
            url.host_str(),
            Some("crates.io" | "registry.npmjs.org" | "pypi.org" | "api.deps.dev")
        )
}

pub(super) async fn get(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let url = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
    if !allowed(&url) {
        return Err("Registry URL is outside fixed HTTPS authorities".into());
    }
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("Metadata request failed: {error}"))?;
    decode(response).await
}

pub(super) async fn decode(response: reqwest::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!(
            "Metadata HTTP {}; redirects are not followed",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE as u64)
    {
        return Err("Metadata response exceeds 2 MiB".into());
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("Read metadata body: {error}"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("Metadata response exceeds 2 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("Invalid registry JSON: {error}"))
}

/// Percent-encode a whole path parameter, including scoped package slashes.
pub(super) fn segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

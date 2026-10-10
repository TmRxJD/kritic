//! Explicit cloud transports. No credential discovery, redirects, tools or retries.
//! Provider output is advisory; the caller validates its domain schema.

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{Client, Url};
use serde_json::{Value, json};

use crate::config::Config;

pub const PROVIDERS: &[&str] = &["openai-compatible", "openai", "anthropic", "gemini", "jev"];
const MAX_RESPONSE: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub struct Completion {
    pub value: Value,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub fn endpoint(cfg: &Config) -> Result<Url, String> {
    if !PROVIDERS.contains(&cfg.cloud_provider.as_str()) {
        return Err("cloud.provider: unsupported provider".into());
    }
    if cfg.cloud_model.trim().is_empty() {
        return Err("cloud.model must name the model your provider supports".into());
    }
    if !(1..=32768).contains(&cfg.cloud_max_tokens) {
        return Err("cloud.max_output_tokens must be in 1..=32768".into());
    }
    let url =
        Url::parse(&cfg.cloud_url).map_err(|_| "cloud.url must be an absolute API endpoint")?;
    if cfg.cloud_provider == "gemini"
        && !url
            .path()
            .ends_with(&format!("/models/{}:generateContent", cfg.cloud_model))
    {
        return Err("Gemini cloud.url must identify exactly cloud.model".into());
    }
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("cloud.url requires HTTPS (HTTP is allowed only on loopback)".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "cloud.url must not contain credentials, a query or a fragment; use key_env/key_file"
                .into(),
        );
    }
    Ok(url)
}

/// Bounded HTTP JSON exchange. Errors omit response bodies and URLs: either may contain secrets.
pub async fn exchange(cfg: &Config, key: &str, body: &Value) -> Result<Value, String> {
    let url = endpoint(cfg)?;
    if body.to_string().len() > 1024 * 1024 {
        return Err("cloud request exceeds the 1 MiB input limit".into());
    }
    let http = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(cfg.kev_request_s))
        .build()
        .map_err(|_| "cannot build cloud HTTP client")?;
    let mut request = http.post(url).json(body);
    request = match cfg.cloud_provider.as_str() {
        "anthropic" => request
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        "gemini" => request.header("x-goog-api-key", key),
        _ => request.bearer_auth(key),
    };
    let read = async {
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                "cloud request timed out"
            } else {
                "cloud request failed (transport/TLS)"
            }
        })?;
        if !response.status().is_success() {
            return Err(format!(
                "cloud HTTP {}{}",
                response.status(),
                if matches!(response.status().as_u16(), 401 | 403) {
                    " (check the configured API key)"
                } else {
                    ""
                }
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE as u64)
        {
            return Err("cloud response exceeds 2 MiB".into());
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "cloud response body failed")?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
                return Err("cloud response exceeds 2 MiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| "cloud returned invalid JSON".into())
    };
    tokio::time::timeout(Duration::from_secs(cfg.kev_request_s), read)
        .await
        .map_err(|_| "cloud request timed out (including response body)".to_string())?
}

/// JSON-only prompt compatibility mode: no dependence on proprietary structured-output flags.
/// Local validation is mandatory even when a provider offers JSON mode.
pub fn request(
    cfg: &Config,
    model: &str,
    system: &str,
    user: &str,
    image: Option<(&str, &str)>,
) -> Result<Value, String> {
    let max = cfg.cloud_max_tokens;
    if cfg.cloud_provider == "gemini" && model != cfg.cloud_model {
        return Err("Gemini vision.cloud_model must match the model in cloud.url".into());
    }
    Ok(match cfg.cloud_provider.as_str() {
        "openai" | "openai-compatible" => {
            let content = match image {
                Some((mime, data)) => json!([
                    {"type": "text", "text": user},
                    {"type": "image_url", "image_url": {"url": format!("data:{mime};base64,{data}")}}
                ]),
                None => json!(user),
            };
            let mut body = json!({"model": model, "stream": false, "messages": [
                {"role": "system", "content": system}, {"role": "user", "content": content}
            ]});
            body[if cfg.cloud_provider == "openai" {
                "max_completion_tokens"
            } else {
                "max_tokens"
            }] = json!(max);
            body
        }
        "anthropic" => {
            let mut content = vec![json!({"type":"text", "text":user})];
            if let Some((mime, data)) = image {
                content.push(json!({"type":"image", "source":{"type":"base64", "media_type":mime, "data":data}}));
            }
            json!({"model":model, "max_tokens":max, "system":system, "messages":[{"role":"user", "content":content}]})
        }
        "gemini" => {
            let mut parts = vec![json!({"text":user})];
            if let Some((mime, data)) = image {
                parts.push(json!({"inlineData":{"mimeType":mime, "data":data}}));
            }
            json!({"systemInstruction":{"parts":[{"text":system}]}, "contents":[{"role":"user", "parts":parts}],
                "generationConfig":{"maxOutputTokens":max, "responseMimeType":"application/json"}})
        }
        _ => return Err("this provider does not support chat/image completion".into()),
    })
}

pub fn decode(provider: &str, raw: &Value) -> Result<Completion, String> {
    let (text, input, output) = match provider {
        "openai" | "openai-compatible" => {
            let choices = raw["choices"]
                .as_array()
                .ok_or("cloud response missing choices")?;
            if choices.len() != 1
                || choices[0]["finish_reason"] != "stop"
                || choices[0]["message"]
                    .get("refusal")
                    .is_some_and(|v| !v.is_null())
                || choices[0]["message"]
                    .get("tool_calls")
                    .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
            {
                return Err("cloud response refused, truncated or returned tool calls".into());
            }
            (
                choices[0]["message"]["content"]
                    .as_str()
                    .ok_or("cloud response missing text")?,
                raw["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
                raw["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            )
        }
        "anthropic" => {
            let blocks = raw["content"]
                .as_array()
                .ok_or("cloud response missing content")?;
            if raw["stop_reason"] != "end_turn" || blocks.len() != 1 || blocks[0]["type"] != "text"
            {
                return Err(
                    "cloud response refused, truncated or returned non-text content".into(),
                );
            }
            (
                blocks[0]["text"]
                    .as_str()
                    .ok_or("cloud response missing text")?,
                raw["usage"]["input_tokens"].as_u64().unwrap_or(0),
                raw["usage"]["output_tokens"].as_u64().unwrap_or(0),
            )
        }
        "gemini" => {
            let candidates = raw["candidates"]
                .as_array()
                .ok_or("cloud response blocked or missing candidates")?;
            if candidates.len() != 1 || candidates[0]["finishReason"] != "STOP" {
                return Err("cloud response blocked or truncated".into());
            }
            let parts = candidates[0]["content"]["parts"]
                .as_array()
                .ok_or("cloud response missing parts")?;
            if parts.len() != 1
                || parts[0].get("functionCall").is_some()
                || parts[0]["thought"] == true
            {
                return Err("cloud response returned non-answer content".into());
            }
            (
                parts[0]["text"]
                    .as_str()
                    .ok_or("cloud response missing text")?,
                raw["usageMetadata"]["promptTokenCount"]
                    .as_u64()
                    .unwrap_or(0),
                raw["usageMetadata"]["candidatesTokenCount"]
                    .as_u64()
                    .unwrap_or(0),
            )
        }
        _ => return Err("unsupported chat provider".into()),
    };
    let value: Value = serde_json::from_str(text)
        .map_err(|_| "cloud model did not return a single JSON object")?;
    if !value.is_object() {
        return Err("cloud model output must be a JSON object".into());
    }
    Ok(Completion {
        value,
        input_tokens: input,
        output_tokens: output,
    })
}

pub async fn complete(
    cfg: &Config,
    model: &str,
    system: &str,
    user: &str,
    image: Option<(&str, &str)>,
) -> Result<Completion, String> {
    let key = cfg.cloud_key().ok_or_else(|| {
        format!(
            "cloud API key missing: set {} or run `kritic key`",
            cfg.cloud_key_env
        )
    })?;
    let body = request(cfg, model, system, user, image)?;
    decode(&cfg.cloud_provider, &exchange(cfg, &key, &body).await?)
}

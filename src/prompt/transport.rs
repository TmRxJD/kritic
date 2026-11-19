//! Text generation belongs to an explicitly selected Qwen/cloud/native provider, never Kev.
use super::*;
use futures_util::StreamExt;
use kritic_prompts::{MAX_INPUT, SYSTEM};

pub(super) async fn generate(cfg: &Config, original: &str) -> Result<Value, String> {
    let packet = json!({"response_schema":schema(),"untrusted_user_prompt":original}).to_string();
    match cfg.prompt_backend.as_str() {
        "cloud" => {
            if cfg.cloud_provider == "jev" {
                return Err("JEV is a typed decision provider, not a prompt generator; choose a text provider".into());
            }
            let model = if cfg.prompt_model.is_empty() {
                &cfg.cloud_model
            } else {
                &cfg.prompt_model
            };
            Ok(crate::provider::complete(cfg, model, SYSTEM, &packet, None)
                .await?
                .value)
        }
        "local" => local(cfg, &packet).await,
        "codex" => {
            kritic_prompts::codex::generate(&cfg.prompt_codex_exe, &cfg.prompt_model, &packet).await
        }
        _ => Err("prompt.backend must be off, local, cloud or codex".into()),
    }
}

async fn local(cfg: &Config, packet: &str) -> Result<Value, String> {
    if cfg.prompt_model.trim().is_empty() {
        return Err("Set prompt.model to an installed text model, such as your small Qwen model; models are never downloaded automatically".into());
    }
    let url = reqwest::Url::parse(&format!(
        "{}/api/chat",
        cfg.ollama_url.trim_end_matches('/')
    ))
    .map_err(|_| "Invalid configured local router URL")?;
    if !matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    ) || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Prompt local backend requires a credential-free loopback Ollama/router URL; use cloud for remote providers".into());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(cfg.prompt_timeout_s))
        .build()
        .map_err(|_| "Cannot create prompt HTTP client")?;
    // The configured shared router owns model residency and GPU scheduling.
    let response=client.post(url).json(&json!({"model":cfg.prompt_model,"stream":false,"format":schema(),
        "options":{"temperature":0,"num_predict":2048},"messages":[{"role":"system","content":SYSTEM},{"role":"user","content":packet}]}))
        .send().await.map_err(|_|"Local prompt provider transport failed")?;
    if !response.status().is_success() {
        return Err(format!("Local prompt provider HTTP {}", response.status()));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Local prompt response failed")?;
        if bytes.len() + chunk.len() > MAX_INPUT * 2 {
            return Err("Local prompt response exceeds 128 KiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let raw: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Local prompt provider returned non-JSON")?;
    if raw["done"] != true {
        return Err("Local prompt generation did not complete".into());
    }
    if raw["done_reason"] == "length"
        || raw["message"]["tool_calls"]
            .as_array()
            .is_some_and(|calls| !calls.is_empty())
    {
        return Err("Local prompt proposal was truncated or attempted tool calls".into());
    }
    let content = raw["message"]["content"]
        .as_str()
        .ok_or("Local prompt provider omitted content")?;
    serde_json::from_str(content).map_err(|_| "Local prompt proposal is not JSON".into())
}

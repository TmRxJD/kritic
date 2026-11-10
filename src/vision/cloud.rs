//! Cloud image request translation; local residency coordination stays in chat.

use super::MAX_VISUAL_REFERENCES;
use crate::config::Config;
use serde_json::{Value, json};

pub(super) async fn chat(cfg: &Config, body: &Value) -> Result<Value, String> {
    let system = body["messages"][0]["content"]
        .as_str()
        .ok_or("vision request missing system prompt")?;
    let user = body["messages"][1]["content"]
        .as_str()
        .ok_or("vision request missing user prompt")?;
    let attachments = body["messages"][1]["images"]
        .as_array()
        .ok_or("vision request missing images")?;
    if attachments.is_empty() || attachments.len() > MAX_VISUAL_REFERENCES + 1 {
        return Err("vision request requires one subject and at most two references".into());
    }
    let mut images = Vec::new();
    for attachment in attachments {
        let data = attachment
            .as_str()
            .ok_or("vision request image is not base64 text")?;
        let mime = if data.starts_with("iVBOR") {
            "image/png"
        } else if data.starts_with("/9j/") {
            "image/jpeg"
        } else if data.starts_with("UklG") {
            "image/webp"
        } else {
            return Err("cloud vision requires PNG, JPEG or WebP".into());
        };
        images.push((mime, data));
    }
    let schema = body.get("format").map(Value::to_string).unwrap_or_default();
    let system = format!("{system} Return JSON only. Required output schema: {schema}");
    let c = crate::provider::complete_images(cfg, &cfg.vision_cloud_model, &system, user, &images)
        .await?;
    crate::usage::log_model_usage(
        cfg,
        json!({"model":cfg.vision_cloud_model,"backend":cfg.cloud_provider,"input_tokens":c.input_tokens,"output_tokens":c.output_tokens}),
    );
    Ok(
        json!({"message":{"content":c.value.to_string()},"prompt_eval_count":c.input_tokens,"eval_count":c.output_tokens}),
    )
}

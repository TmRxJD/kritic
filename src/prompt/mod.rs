//! Prompt ingress is advisory and independent of end-of-task implementation review.
mod transport;
use crate::config::Config;
use serde_json::{Value, json};
use std::time::Duration;

pub use kritic_prompts::{hook_context, schema};

/// One bounded attempt with explicit provider selection; never silently changes provider.
pub async fn review(cfg: &Config, original: &str, hook: bool) -> Result<Value, String> {
    let mut report = kritic_prompts::evaluate(original, None)?;
    report["provider"] = json!({"backend":cfg.prompt_backend,"model":cfg.prompt_model});
    if !cfg.config_errors.is_empty() {
        report["provider_error"] = json!("Prompt model disabled until config_errors are fixed");
        report["config_errors"] = json!(cfg.config_errors);
        report["next_step"] = json!(
            "Run the matching kritic doctor --json, inspect config_errors, and repair the named keys before retrying; original task remains available"
        );
        return Ok(report);
    }
    if !cfg.tool_enabled("prompt_critic") || cfg.prompt_backend == "off" {
        return Ok(report);
    }
    let seconds = if hook {
        cfg.prompt_timeout_s.min(8)
    } else {
        cfg.prompt_timeout_s
    };
    let generated = tokio::time::timeout(
        Duration::from_secs(seconds),
        transport::generate(cfg, original),
    )
    .await;
    let value = match generated {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            report["provider_error"] = json!(error);
            return Ok(report);
        }
        Err(_) => {
            report["provider_error"] = json!(format!(
                "Prompt review timed out after {seconds}s; original retained"
            ));
            return Ok(report);
        }
    };
    match kritic_prompts::evaluate(original, Some(value)) {
        Ok(mut result) => {
            result["provider"] = report["provider"].clone();
            Ok(result)
        }
        Err(error) => {
            report["provider_error"] = json!(error);
            Ok(report)
        }
    }
}

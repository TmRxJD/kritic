//! Prompt intake is explicitly user-enrolled, not an implementation approval hook.
use super::{Kritic, audit, finished, started};
use rmcp::{Json, handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Params {
    prompt: String,
}

#[tool_router(router=prompts_router,vis="pub(super)")]
impl Kritic {
    #[tool(name = "prompt_critic")]
    async fn prompt_critic(
        &self,
        Parameters(params): Parameters<Params>,
    ) -> Result<Json<Value>, String> {
        if !self.kev.cfg.tool_enabled("prompt_critic") {
            return Err(
                "Enable capabilities.prompts explicitly; model selection is separate in [prompt]"
                    .into(),
            );
        }
        let start = started("prompt_critic");
        let report = crate::prompt::review(&self.kev.cfg, &params.prompt, false).await?;
        // Do not put original private user prompts into the ordinary event log.
        audit(
            &self.kev.cfg,
            "prompt_critic",
            &serde_json::json!({"original_sha256":report["original_sha256"],"status":report["status"]}),
        );
        finished("prompt_critic", start, &report);
        Ok(Json(report))
    }
}

//! Opt-in session/continuity review. Controllers, not model scores, own lifecycle changes.
use super::{Kritic, audit, finished, started};
use rmcp::{Json, handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Params {
    base_dir: String,
    case_path: Option<String>,
    /// Optional lossless archive/typed relevance advice; protected user/pinned facts survive.
    #[serde(default)]
    curate: bool,
    #[serde(default)]
    use_kev: bool,
}

#[tool_router(router = sessions_router, vis = "pub(super)")]
impl Kritic {
    /// Evaluate explicit operational limits against pinned session evidence; never interrupts the current turn.
    #[tool(name = "session_critic")]
    async fn session_critic(
        &self,
        Parameters(params): Parameters<Params>,
    ) -> Result<Json<Value>, String> {
        evaluate(self, params, false).await.map(Json)
    }

    /// Check protected context coverage, current fixtures and workspace before a controller prepares fresh context.
    #[tool(name = "handoff_critic")]
    async fn handoff_critic(
        &self,
        Parameters(params): Parameters<Params>,
    ) -> Result<Json<Value>, String> {
        if params.curate || params.use_kev {
            return Err("Use session_critic for optional archive advice; handoff validation is deterministic".into());
        }
        evaluate(self, params, true).await.map(Json)
    }
}

async fn evaluate(server: &Kritic, params: Params, handoff: bool) -> Result<Value, String> {
    let name = if handoff {
        "handoff_critic"
    } else {
        "session_critic"
    };
    if !server.kev.cfg.tool_enabled(name) {
        return Err("Enable capabilities.sessions explicitly; automatic rollover enrollment is a separate opt-in".into());
    }
    if params.use_kev && !params.curate {
        return Err(
            "use_kev requires curate=true; deterministic review never silently invokes a model"
                .into(),
        );
    }
    let start = started(name);
    let request = crate::continuity::Request {
        base_dir: params.base_dir,
        case_path: params.case_path,
    };
    let copy = request.clone();
    let mut report = tokio::task::spawn_blocking(move || {
        if handoff {
            crate::continuity::handoff_critic(&copy)
        } else {
            crate::continuity::session_critic(&copy)
        }
    })
    .await
    .map_err(|e| format!("Continuity worker: {e}"))??;
    if params.curate {
        crate::continuity::evidence::git(std::path::Path::new(&request.base_dir),&["check-ignore","--",".kritic/session-state/"])
            .map_err(|_|"Before curation, explicitly ignore permanent .kritic/session-state/ in the project; do not write private history into tracked files")?;
        let copy = request.clone();
        let (case, messages) =
            tokio::task::spawn_blocking(move || crate::continuity::curation_input(&copy))
                .await
                .map_err(|e| e.to_string())??;
        let protected: Vec<_> = case
            .facts
            .iter()
            .filter(|f| !matches!(f.kind, crate::continuity::FactKind::Context))
            .flat_map(|f| {
                std::iter::once(f.event_id.clone())
                    .chain(f.solution.as_ref().map(|s| s.event_id.clone()))
            })
            .collect();
        let input: crate::context::CompressInput = serde_json::from_value(json!({"messages":messages,
            "target_tokens":case.policy.fresh_context_max_bytes / 4,"session_id":case.session_id,
            "focus":format!("{}\nOverarching requirements: {:?}",case.goal,case.overarching_requests),
            "protect_first_n":0,"protect_last_n":2,"protect_user_messages":true,"protected_event_ids":protected,
            "mode":"compress","use_kev":params.use_kev,"rank_max":16,"rank_deadline_s":5})).map_err(|e|e.to_string())?;
        let archive = crate::context::Archive::for_session(
            &std::path::Path::new(&request.base_dir).join(".kritic/session-state"),
            &case.session_id,
        );
        let compressed = crate::context::compress(
            if params.use_kev {
                Some(server.kev.as_ref())
            } else {
                None
            },
            &archive,
            &input,
        )
        .await;
        report["curation"] = serde_json::to_value(compressed).map_err(|e| e.to_string())?;
        report["curation_warning"] = json!(
            "Token counts are character estimates; curation is advisory and not a transfer receipt. Raw archived content remains retrievable."
        );
        let after = crate::continuity::session_critic(&request)?;
        if after["session_case_sha256"] != report["session_case_sha256"]
            || after["findings"] != json!([])
        {
            return Err("Continuity evidence changed while curating; preserve archive and prepare a new pinned snapshot".into());
        }
    }
    let full = crate::tools::save_full(&server.kev.cfg, name, &report)
        .ok_or("Cannot persist full continuity review; repair runs_dir before continuing")?;
    report
        .as_object_mut()
        .ok_or("Expected object report")?
        .remove("curated_context");
    if let Some(curation) = report.get_mut("curation").and_then(Value::as_object_mut) {
        curation.remove("messages");
    }
    report["full_result"] = json!(full);
    finished(name, start, &report);
    audit(&server.kev.cfg, name, &report);
    Ok(report)
}

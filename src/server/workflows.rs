//! Opt-in discovery and execution for registered read-only workflows.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use rmcp::{Json, handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    Kritic, audit, finished,
    git_ci::{GitCiParams, GitCiSnapshot, snapshot},
    started,
};

const WORKFLOW_NAME: &str = "git_ci_snapshot";
const WORKFLOW_SUMMARY: &str = "Read status in a trusted Git checkout and summarize a GitHub pull request (PR)'s checks and failed job log.";

#[tool_router(router = workflows_router, vis = "pub(super)")]
impl Kritic {
    /// Find read-only workflows. Set name or request schemas for a keyword query.
    #[tool(name = "workflow_discover")]
    async fn workflow_discover(
        &self,
        Parameters(params): Parameters<DiscoverParams>,
    ) -> Result<Json<WorkflowDiscovery>, String> {
        Ok(Json(WorkflowDiscovery {
            workflows: discover(params)?,
        }))
    }

    /// Run one registered read-only workflow. Unknown names and invalid arguments are rejected.
    #[tool(name = "workflow_run")]
    async fn workflow_run(
        &self,
        Parameters(params): Parameters<RunParams>,
    ) -> Result<Json<GitCiSnapshot>, String> {
        let params = workflow_args(params)?;
        let start = started("workflow_run");
        let result = snapshot(params).await;
        finished("workflow_run", start, &result);
        audit(&self.kev.cfg, "workflow_run", &result);
        let result = result?;
        Ok(Json(result))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DiscoverParams {
    /// Filter names and descriptions with keywords.
    query: Option<String>,
    /// Exact workflow name. Cannot be combined with query.
    name: Option<String>,
    /// Include exact argument schemas for a nonempty keyword query. Name always returns its schema.
    #[serde(default)]
    include_schema: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunParams {
    /// Exact workflow name.
    name: String,
    /// Arguments matching the named workflow's input schema.
    arguments: Map<String, Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct Workflow {
    name: &'static str,
    summary: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<Map<String, Value>>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkflowDiscovery {
    workflows: Vec<Workflow>,
}

fn discover(params: DiscoverParams) -> Result<Vec<Workflow>, String> {
    if params.name.is_some() && params.query.is_some() {
        return Err("choose either name or query".into());
    }
    let mut workflow = Workflow {
        name: WORKFLOW_NAME,
        summary: WORKFLOW_SUMMARY,
        input_schema: None,
    };
    if let Some(name) = &params.name {
        if name != WORKFLOW_NAME {
            return Err("unknown read-only workflow".into());
        }
    } else if let Some(query) = &params.query {
        if query.len() > 160 {
            return Err("query must be at most 160 bytes".into());
        }
        let summary = WORKFLOW_SUMMARY.to_ascii_lowercase();
        if !query
            .to_ascii_lowercase()
            .split_whitespace()
            .all(|word| WORKFLOW_NAME.contains(word) || summary.contains(word))
        {
            return Ok(Vec::new());
        }
    }
    if params.include_schema
        && params.name.is_none()
        && params
            .query
            .as_deref()
            .is_none_or(|query| query.trim().is_empty())
    {
        return Err("include_schema requires a nonempty query or exact name".into());
    }
    if params.name.is_some() || params.include_schema {
        let mut schema = schemars::schema_for!(GitCiParams)
            .as_object()
            .cloned()
            .ok_or("workflow argument schema must be an object")?;
        crate::tool_metadata::compact_schema(&mut schema);
        workflow.input_schema = Some(schema);
    }
    Ok(vec![workflow])
}

fn workflow_args(params: RunParams) -> Result<GitCiParams, String> {
    if params.name != WORKFLOW_NAME {
        return Err("unknown read-only workflow".into());
    }
    if params.arguments.len() > 2 {
        return Err("git_ci_snapshot accepts cwd and optional pull_request".into());
    }
    serde_json::from_value(Value::Object(params.arguments)).map_err(|e| {
        let detail: String = e.to_string().chars().take(240).collect();
        format!("invalid workflow arguments: {detail}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_argument_schema_is_available_by_name_only() {
        let list = discover(DiscoverParams {
            query: Some("git status".into()),
            name: None,
            include_schema: false,
        })
        .unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].input_schema.is_none());
        let exact = discover(DiscoverParams {
            query: None,
            name: Some(list[0].name.into()),
            include_schema: false,
        })
        .unwrap();
        let schema = exact[0].input_schema.as_ref().unwrap();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], serde_json::json!(["cwd"]));
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn pull_request_query_can_return_schema_in_one_call() {
        let matches = discover(DiscoverParams {
            query: Some("pull request".into()),
            name: None,
            include_schema: true,
        })
        .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, WORKFLOW_NAME);
        assert!(matches[0].input_schema.is_some());
    }

    #[test]
    fn workflow_rejects_unknown_name_and_executable_arguments() {
        let unknown = RunParams {
            name: "cli_run".into(),
            arguments: Map::new(),
        };
        assert!(workflow_args(unknown).is_err());
        let run = RunParams {
            name: WORKFLOW_NAME.into(),
            arguments: serde_json::json!({"cwd": ".", "program": "git"})
                .as_object()
                .unwrap()
                .clone(),
        };
        assert!(workflow_args(run).is_err());
    }
}

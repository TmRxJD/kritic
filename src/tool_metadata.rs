//! Compact discovery metadata without changing tools or JSON Schema validation.
use serde_json::{Map, Value};

// Some clients repeat these instructions beside every tool. Keep workflow detail in the skill.
pub(crate) const INSTRUCTIONS: &str = "TEST RELEASE: independently validate findings; reproduce and fix Kritic defects with regressions and PRs. Advisory only. Run scoped deterministic checks first; models are optional. Never fabricate scores or execution evidence.";

pub(crate) fn description(name: &str) -> Option<&'static str> {
    Some(match name {
        "status" => {
            "Check capabilities, configuration and reviewer health. start=true may start an owned local reviewer; external services are never started."
        }
        "artifact_validate" => {
            "Validate files and run explicit project commands verbatim. Failures block approval; skipped checks remain gaps. Models optional."
        }
        "contract_critic" => {
            "Audit work contracts against scoped checks and evidence. Shell commands require run_commands=true; prose-only promises remain unverified."
        }
        "repository_critic" => {
            "Check staged commit scope/size and repository organization policy. Static approval cannot establish semantic cohesion or comment quality."
        }
        "review_history" => {
            "Record, list, compare or export explicit review snapshots. Historical evidence is unverified and never approves current work. Exports may contain private data."
        }
        "technical_debt_critic" => {
            "Review dependencies and declared maintenance. Public registry requests require online=true. Urgency/effort are estimates; no upgrades are applied."
        }
        "framing_critic" => {
            "Check a debugging case against pinned observations, pipeline and hypotheses. No commands or models run. Consistency is not proof of root cause; inspect full_result."
        }
        "critic_score" => {
            "Optional model review of diffs or files. Prefer git_diff for uncommitted changes. Verify flagged locations; uncalibrated scores cannot approve work."
        }
        "decide" => {
            "Optional typed model decision with uncertainty. Escalation means decide independently; probabilities are not proof."
        }
        "visual_critic" => {
            "Optional screenshot craft assessment. Verify observations against actual UI; the rubric is uncalibrated and approval is always null."
        }
        "ui_test" => {
            "Browser interaction, persistence, error and accessibility checks. Destructive-looking controls require opt-in. Functional success does not prove visual quality."
        }
        "ux_critic" => {
            "Audit workflows and UI rules against .kritic/ux.toml. Inspect evidence and critic_brief; verify model observations independently."
        }
        "security_critic" => {
            "Run installed secret, dependency and code scanners. Missing scanners are gaps; high/critical findings block approval regardless of model triage."
        }
        "code_quality_critic" => {
            "Run linters and code metrics on scoped files. Optional model observations require verification; static metrics do not prove good design."
        }
        "desktop_test" => {
            "Launch and test a Windows app via UI Automation or Unity events. Check controls, errors and persistence; custom rendering may limit coverage."
        }
        "unity_tests" => {
            "Run Unity tests headlessly and parse NUnit results. Requires the matching editor; close other editors for the project first."
        }
        "resource_critic" => {
            "Launch a command, or explicitly attach to a PID, and sample resource use. Trust measured budgets, not model leak opinions; no per-frame timing."
        }
        "hygiene_critic" => {
            "Inspect project processes, sockets and leftovers. clean=true kills identified orphans/socket inheritors; files are only reported. Review scope first."
        }
        "plugin_list" => {
            "List evaluator manifests in <base_dir>/.kritic/plugins. Inspect commands and schemas before running a plugin."
        }
        "plugin_run" => {
            "Execute a trusted repository plugin with validated JSON input. Inspect its command first. Results and calibrated gates are advisory evidence."
        }
        "spec_lint" => {
            "Validate decision specs, question definitions and calibration evidence. builtin lists specs; with_schema returns the schema."
        }
        "token_metrics" => {
            "Measure supported transcript/context and local-model usage. Missing or estimated accounting is not exact billing."
        }
        "route" => {
            "Optional typed Codex model-tier advice. Low confidence or errors keep the standard tier; never override user routing."
        }
        "context_trim" => {
            "Rank supplied chunks and retain relevant text within explicit budgets. Extractive only; no summary generation."
        }
        "locate" => {
            "Rank repository files for a task using keyword prefiltering and optional model relevance. Suggested requirements need independent verification."
        }
        "map_eval" => {
            "Check Beat Saber map structure and heuristic metrics, with optional model review. A static pass does not establish musical or gameplay quality."
        }
        _ => return None,
    })
}

/// Visit schema nodes, not arbitrary JSON. A property named `title` or an enum/const payload
/// must remain intact; annotation keywords only have meaning at a schema node.
pub(crate) fn compact_schema(schema: &mut Map<String, Value>) {
    schema.remove("title");
    schema.remove("examples");
    if schema.get("default").is_some_and(|value| {
        value.is_null()
            || value.as_str() == Some("")
            || value.as_array().is_some_and(Vec::is_empty)
            || value.as_object().is_some_and(Map::is_empty)
    }) {
        schema.remove("default");
    }
    // Preserve descriptions, nonempty defaults, dialects, references and every validation key.
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(Value::Object(children)) = schema.get_mut(key) {
            for child in children.values_mut() {
                compact_node(child);
            }
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(Value::Array(children)) = schema.get_mut(key) {
            for child in children {
                compact_node(child);
            }
        }
    }
    for key in [
        "additionalProperties",
        "unevaluatedProperties",
        "items",
        "additionalItems",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "unevaluatedItems",
        "contentSchema",
    ] {
        if let Some(child) = schema.get_mut(key) {
            compact_node(child);
        }
    }
    // Draft-07 permits tuple-form items; leave its validation structure unchanged too.
    if let Some(Value::Array(children)) = schema.get_mut("items") {
        for child in children {
            compact_node(child);
        }
    }
}

fn compact_node(node: &mut Value) {
    if let Value::Object(schema) = node {
        compact_schema(schema);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn removes_only_redundant_annotations_at_schema_nodes() {
        let mut schema = json!({
            "$schema":"https://json-schema.org/draft/2020-12/schema", "title":"Request",
            "type":"object", "required":["title"], "additionalProperties":false,
            "properties":{
                "title":{"title":"Title", "type":"string", "minLength":1, "description":"Required name"},
                "default":{"type":"object", "default":{"title":"user data"}},
                "nullable":{"anyOf":[{"type":"string","title":"Text"},{"type":"null"}],"default":null},
                "action":{"type":"string","enum":["title","examples"],"default":"list"},
                "run_commands":{"type":"boolean","default":false},
                "budget":{"type":"integer","minimum":1,"maximum":100,"default":10}
            },
            "$defs":{"title":{"title":"Named schema", "const":{"title":"literal", "examples":[1]}}},
            "examples":[{"title":"example"}]
        });
        compact_schema(schema.as_object_mut().unwrap());
        assert!(schema.get("title").is_none() && schema.get("examples").is_none());
        assert_eq!(schema["required"], json!(["title"]));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["title"],
            json!({"type":"string","minLength":1,"description":"Required name"})
        );
        assert_eq!(
            schema["properties"]["default"]["default"],
            json!({"title":"user data"})
        );
        assert_eq!(
            schema["$defs"]["title"]["const"],
            json!({"title":"literal","examples":[1]})
        );
        assert_eq!(schema["properties"]["action"]["default"], "list");
        assert_eq!(
            schema["properties"]["action"]["enum"],
            json!(["title", "examples"])
        );
        assert_eq!(schema["properties"]["run_commands"]["default"], false);
        assert_eq!(schema["properties"]["budget"]["maximum"], 100);
        assert!(schema["properties"]["nullable"].get("default").is_none());
        assert_eq!(schema["properties"]["nullable"]["anyOf"][1]["type"], "null");
        assert!(schema.get("$schema").is_some());
        let once = schema.clone();
        compact_schema(schema.as_object_mut().unwrap());
        assert_eq!(schema, once, "compaction must be idempotent");
    }

    #[test]
    fn descriptions_cover_every_registered_tool_and_keep_safety_opt_ins() {
        assert!(!crate::config::TOOL_NAMES.is_empty());
        for name in crate::config::TOOL_NAMES {
            let description =
                description(name).unwrap_or_else(|| panic!("missing description: {name}"));
            assert!(
                !description.is_empty() && description.len() <= 200,
                "{name}"
            );
        }
        assert!(
            description("contract_critic")
                .unwrap()
                .contains("run_commands=true")
        );
        assert!(
            description("technical_debt_critic")
                .unwrap()
                .contains("online=true")
        );
        assert!(description("hygiene_critic").unwrap().contains("kills"));
        assert!(description("plugin_run").unwrap().contains("trusted"));
        assert!(INSTRUCTIONS.contains("TEST RELEASE") && INSTRUCTIONS.contains("independently"));
        assert!(description("unknown_extension").is_none());
    }
}

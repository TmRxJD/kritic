//! Read-only, project-scoped setup instructions. Presence is never execution evidence.
use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use crate::config::{Config, TOOL_NAMES};

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Recovery {
    pub code: String,
    pub steps: Vec<String>,
    pub guide: String,
}

pub fn contract_recovery() -> Recovery {
    Recovery {
        code: "contract_setup_required".into(),
        steps: vec![
            "Confirm base_dir is the target repository root and contract_path is inside it; do not change global model settings for a missing project file.".into(),
            "Read that repository's AGENTS.md, ownership rules, manifests, validation scripts and task requirements. Check for an existing contract at another in-repository path first.".into(),
            "If missing, create .kritic/contract.toml version=1 with real required acceptance checks and actual CI/hook bindings. Use examples/contract.toml for syntax, not as an unchanged passing template. Retain uncovered prose requirements as unverified.".into(),
            "If present but invalid, repair its reported parse/path/size error without overwriting unrelated requirements or weakening checks.".into(),
            "Rerun contract_critic with the same base_dir and run_commands=false. Review commands before explicitly opting in; skipped commands and missing evidence cannot approve work.".into(),
        ],
        guide: "docs/contracts.md".into(),
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ToolSetup {
    pub tool: String,
    pub enabled: bool,
    pub capability: Option<String>,
    pub project_config: Option<String>,
    pub config_state: String,
    pub required: bool,
    pub next_step: String,
    pub guide: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProjectSetup {
    pub base_dir: String,
    pub scope: String,
    pub common_steps: Vec<String>,
    pub tools: Vec<ToolSetup>,
}

struct Guide {
    capability: Option<&'static str>,
    manifest: Option<&'static str>,
    required: bool,
    document: &'static str,
    action: &'static str,
}

fn guide(tool: &str) -> Result<Guide, String> {
    let (capability, manifest, required, document, action) = match tool {
        "status" => (
            None,
            None,
            false,
            "configuration.md",
            "Inspect config_errors (omitted when empty), effective capabilities and reviewer health. Fix only supported configuration keys, preserve external service ownership, then reconnect MCP after configuration changes.",
        ),
        "artifact_validate" => (
            None,
            None,
            false,
            "tools.md",
            "Supply actual changed paths and reviewed project validation commands. use_model=false needs no reviewer; parser success alone is not behavioral validation.",
        ),
        "contract_critic" => (
            Some("contracts"),
            Some(".kritic/contract.toml"),
            true,
            "contracts.md",
            "Create a version-1 contract from this project's actual rules, commands and enforcement. Read existing rules first; do not copy generic requirements unchanged. Call run_commands=false first; explicitly opt in only after reviewing commands.",
        ),
        "repository_critic" => (
            Some("repository"),
            Some(".kritic/repository.toml"),
            false,
            "repository-policy.md",
            "Use Git and define repository-owned naming/layer/LoC and commit rules when needed. focused is the default; broad/atomic are choices. Inspect staged scope. Installing hooks is separate and explicit; never replace an existing hook setup silently.",
        ),
        "review_history" => (
            Some("history"),
            None,
            false,
            "memory-adapter.md",
            "Select a real saved report, exact input hashes and private storage for an explicit record action. No model/indexing is required. History and jcode-format export never provide fresh approval.",
        ),
        "technical_debt_critic" => (
            Some("debt"),
            Some(".kritic/debt.toml"),
            false,
            "technical-debt.md",
            "Use actual dependency manifests/locks; optionally declare unsupported dependencies and maintenance evidence. Keep online=false unless public registry disclosure is authorized. Missing cached versions are unknown, not current.",
        ),
        "staleness_critic" => (
            Some("staleness"),
            Some(".kritic/test-contracts.toml"),
            false,
            "staleness.md",
            "Use kritic staleness --schema. Derive explicit test roots/pattern, canonical owners, test/input review hashes and fixture consumers from actual repository contracts. CLI check is read-only; sync is dry-run unless --write. Never auto-refresh review hashes or rewrite assertions to match broken code.",
        ),
        "invariant_critic" => (
            Some("invariants"),
            Some(".kritic/test-contracts.toml"),
            false,
            "staleness.md",
            "Declare key boundaries, authority ownership, required invariants, rationale, release guards, failure policies and actual negative-test bindings in the same manifest. Use kritic staleness invariants. Missing declarations or textual matches never prove sensible invariants or real execution.",
        ),
        "framing_critic" => (
            Some("framing"),
            Some(".kritic/debug-case.json"),
            true,
            "framing.md",
            "Use kritic framing --schema and examples/debug-case.json for structure. Create a case from actual reproduction, pinned observations, configuration ownership and hypotheses; synthetic example evidence cannot diagnose this project.",
        ),
        "critic_score" | "decide" => (
            None,
            None,
            false,
            "configuration.md",
            "Optional model feature: choose supported kev.backend settings for an already-authorized local or cloud reviewer. A coding-agent subscription is not a separate API key. Fix config_errors first, then verify health; do not start external services, download models or change GPU placement just to obtain a score.",
        ),
        "visual_critic" => (
            None,
            None,
            false,
            "visual-quality.md",
            "Optional vision feature: configure vision.backend with an authorized image-capable provider, then supply a current image or a reachable app/browser. Keep disabled without vision. Inspect the screenshot and rubric yourself; model approval is uncalibrated.",
        ),
        "ui_test" => (
            Some("web"),
            None,
            false,
            "tools.md",
            "Provide a reachable local app and Chromium browser (ui.browser if discovery fails); provision the browser harness only if needed. Declare meaningful flows/persistence. Remote URLs and destructive controls require their explicit opt-ins.",
        ),
        "ux_critic" => (
            Some("ux"),
            Some(".kritic/ux.toml"),
            true,
            "ux-critic.md",
            "Choose examples/ux/web.ux.toml or unity.ux.toml to match the surface. Replace example routes/selectors/tasks with this app's real workflows. Supply a running app; use novice=off for model-free checks. Unsupported surfaces are inapplicable, not broken.",
        ),
        "security_critic" => (
            Some("security"),
            None,
            false,
            "getting-started.md",
            "Install/configure gitleaks, osv-scanner, Semgrep and Bandit; add cargo-audit for Rust. Set scan.tools_bin/scan.cargo_tools_bin as needed and inspect doctor --json. Use triage=false without a model. Missing scanners remain coverage gaps.",
        ),
        "code_quality_critic" => (
            Some("quality"),
            Some(".kritic/repository.toml"),
            false,
            "repository-policy.md",
            "Use the project's real lint/typecheck tools and source paths; optional repository policy defines naming/layers/LoC. Kritic does not install the project's toolchain. Use use_kev=false without a reviewer.",
        ),
        "desktop_test" => (
            Some("desktop"),
            None,
            false,
            "getting-started.md",
            "Requires Windows, an interactive desktop, Python/uv and the packaged desktop harness. Configure desktop.uv/desktop.harness_py, provide the actual executable and meaningful flows. Do not treat unsupported custom-drawn controls as tested.",
        ),
        "unity_tests" => (
            Some("desktop"),
            Some("ProjectSettings/ProjectVersion.txt"),
            true,
            "unity.md",
            "Only applicable to a Unity project. Use its matching installed Editor (unity.editor if discovery fails) and real Test Framework tests. A Rust/web project should skip this critic, not install Unity to make every critic run.",
        ),
        "resource_critic" => (
            Some("resources"),
            None,
            false,
            "tools.md",
            "Supply an authorized launch command or explicit allow_attach=true PID, workload, duration and measured budgets. Preserve shared processes; verify process identity. Model leak opinions are optional, not measured resource violations.",
        ),
        "hygiene_critic" => (
            Some("resources"),
            None,
            false,
            "tools.md",
            "Supply exact project roots and owned run information. Inspect with clean=false; do not kill shared services or unrelated processes to clear a finding.",
        ),
        "plugin_list" | "plugin_run" => (
            Some("plugins"),
            Some(".kritic/plugins"),
            false,
            "plugins.md",
            "No plugins is a valid state. Define a project evaluator manifest only when needed, inspect its executable/commands and dependencies, then select its actual plugin ID. Plugin execution is not implied by listing.",
        ),
        "spec_lint" => (
            None,
            None,
            false,
            "tools.md",
            "Use the specs bundled with this exact binary version. Repair invalid specs with a regression; never relax gates or invent calibration to obtain approval.",
        ),
        "token_metrics" => (
            None,
            None,
            false,
            "tools.md",
            "Provide a supported transcript/session only when token measurements are needed. Unavailable accounting is not zero usage or exact billing; avoid exporting private transcripts.",
        ),
        "route" | "context_trim" | "locate" => (
            Some("context"),
            None,
            false,
            "configuration.md",
            "Optional context helpers need the selected reviewer and explicit text/query inputs. They do not replace task ownership or routing policy; keep disabled when unavailable. Do not index an unspecified corpus.",
        ),
        "map_eval" => (
            Some("domain_maps"),
            None,
            false,
            "tools.md",
            "Only use for supported Beat Saber map content/path and relevant metadata; it is not a general game-map critic. Keep disabled or skip for unrelated projects. Model slices are optional.",
        ),
        _ => return Err(format!("No setup guide registered for tool {tool}")),
    };
    Ok(Guide {
        capability,
        manifest,
        required,
        document,
        action,
    })
}

/// Inspects only fixed prerequisite paths; never reads project contents or creates files.
pub fn inspect(cfg: &Config, base_dir: &str) -> Result<ProjectSetup, String> {
    let base = Path::new(base_dir)
        .canonicalize()
        .map_err(|e| format!("Project setup base_dir: {e}"))?;
    if !base.is_dir() {
        return Err("Project setup base_dir must be a directory".into());
    }
    let mut tools = Vec::new();
    for tool in TOOL_NAMES {
        let g = guide(tool)?;
        let enabled = cfg.tool_enabled(tool);
        let config_state = if !enabled {
            "disabled"
        } else if let Some(relative) = g.manifest {
            let path = base.join(relative);
            match path.canonicalize() {
                Ok(resolved) if !resolved.starts_with(&base) => "outside_project",
                Ok(resolved)
                    if resolved.is_file()
                        || (*tool == "plugin_list" || *tool == "plugin_run")
                            && resolved.is_dir() =>
                {
                    "present_unvalidated"
                }
                Ok(_) => "wrong_path_type",
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
                Err(_) => "inaccessible",
            }
        } else {
            "runtime_inputs_required"
        };
        let next_step = if !enabled {
            let toggle = g.capability.map_or_else(
                || match *tool {
                    "critic_score" | "decide" => "select the authorized kev.backend".into(),
                    "visual_critic" => "select the authorized vision.backend".into(),
                    _ => "remove this tool from tools.disabled".into(),
                },
                |c| format!("set capabilities.{c}=true"),
            );
            format!(
                "Disabled is expected when irrelevant. Only if required for this task, {toggle}, inspect tools.disabled, and reconnect MCP. {}",
                g.action
            )
        } else {
            g.action.into()
        };
        tools.push(ToolSetup {
            tool: (*tool).into(),
            enabled,
            capability: g.capability.map(String::from),
            project_config: g.manifest.map(String::from),
            config_state: config_state.into(),
            required: g.required,
            next_step,
            guide: format!("docs/{}", g.document),
        });
    }
    Ok(ProjectSetup {
        base_dir: base.display().to_string(),
        scope: "Setup guidance and fixed-path presence only. Config contents, installed dependency health, live inputs and execution are unverified. No files, processes, models, network or hooks were changed.".into(),
        common_steps: vec![
            "Use the actual target repository root. Select relevant critics, not every available critic; disabled optional features are not failures.".into(),
            "Run the matching executable's doctor --json for dependency diagnostics. Review checks[].fix and setup --help before installing only needed dependencies; preserve existing model, GPU, credential and external-service ownership settings.".into(),
            "Within the authorized task, repair missing project configuration from repository rules and real validation commands, then rerun the affected critic. Ask only for genuinely missing requirements, credentials, resource authorization or an ambiguous target.".into(),
            "Use references/setup.md in the installed skill or matching source documentation. Reconnect after global configuration/binary changes; project manifest changes are reread by the affected critic. A missing manifest is setup required, not a reason to report the critic itself offline.".into(),
        ], tools,
    })
}

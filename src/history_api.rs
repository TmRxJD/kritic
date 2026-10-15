//! The optional memory boundary: explicit snapshots, bounded responses, no verification reuse.
use crate::config::Config;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
// Every action is an object; MCP requires this explicit root type alongside `oneOf`.
#[schemars(extend("type" = "object"))]
pub enum Request {
    Record {
        base_dir: String,
        report: String,
        /// Exact repository-relative input files; no automatic workspace indexing.
        inputs: Vec<String>,
        /// Caller-declared tool/model/command/policy versions; stored as a fingerprint.
        context: String,
    },
    List {
        base_dir: String,
        #[serde(default)]
        offset: usize,
    },
    Compare {
        base_dir: String,
        left: String,
        right: String,
    },
    ExportJcode {
        base_dir: String,
    },
}

pub fn execute(cfg: &Config, request: Request) -> Result<Value, String> {
    if !cfg.capability("history") || !cfg.tool_enabled("review_history") {
        return Err(
            "history capability is disabled; enable capabilities.history explicitly".into(),
        );
    }
    let store = crate::history::Store::new(&cfg.runs_dir.join("history"));
    match request {
        Request::Record {
            base_dir,
            report,
            inputs,
            context,
        } => {
            let inputs: Vec<PathBuf> = inputs.into_iter().map(PathBuf::from).collect();
            let record =
                store.record(Path::new(&base_dir), Path::new(&report), &inputs, &context)?;
            serde_json::to_value(record.summary()).map_err(|e| e.to_string())
        }
        Request::List { base_dir, offset } => {
            let entries = store.list(Path::new(&base_dir))?;
            let total = entries.len();
            let page: Vec<_> = entries.into_iter().skip(offset).take(25).collect();
            let end = offset.saturating_add(page.len());
            Ok(
                json!({"historical_unverified": true, "total": total, "offset": offset,
                "next_offset": if end < total { Some(end) } else { None }, "entries": page}),
            )
        }
        Request::Compare {
            base_dir,
            left,
            right,
        } => serde_json::to_value(store.compare(Path::new(&base_dir), &left, &right)?)
            .map_err(|e| e.to_string()),
        Request::ExportJcode { base_dir } => {
            let export = store.export_jcode(Path::new(&base_dir))?;
            let count = export["entries"].as_array().map_or(0, Vec::len);
            let path = crate::tools::save_full(cfg, "history-jcode-export", &export)
                .ok_or("Could not save jcode-format export")?;
            Ok(
                json!({"historical_unverified": true, "entries": count, "export_path": path,
                "live_jcode_modified": false}),
            )
        }
    }
}

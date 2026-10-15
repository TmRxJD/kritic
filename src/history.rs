//! Optional private review snapshots. Historical reports never replace current verification.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

mod disk;
mod snapshot;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputHash {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub repository_sha256: String,
    pub revision: Option<String>,
    pub tool: String,
    /// Declared tool/model/command/config provenance, not independently attested.
    pub context_sha256: String,
    pub inputs: Vec<InputHash>,
    pub report_sha256: String,
    pub report: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub id: String,
    pub created_at: String,
    pub snapshot: Snapshot,
    pub integrity_sha256: String,
}

impl Record {
    pub fn summary(&self) -> RecordSummary {
        RecordSummary {
            id: self.id.clone(),
            created_at: self.created_at.clone(),
            tool: self.snapshot.tool.clone(),
            revision: self.snapshot.revision.clone(),
            context_sha256: self.snapshot.context_sha256.clone(),
            report_sha256: self.snapshot.report_sha256.clone(),
            input_count: self.snapshot.inputs.len(),
            inputs_sha256: snapshot::hash(
                serde_json::json!(&self.snapshot.inputs)
                    .to_string()
                    .as_bytes(),
            ),
            historical_unverified: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RecordSummary {
    pub id: String,
    pub created_at: String,
    pub tool: String,
    pub revision: Option<String>,
    pub context_sha256: String,
    pub report_sha256: String,
    pub input_count: usize,
    pub inputs_sha256: String,
    pub historical_unverified: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Difference {
    pub pointer: String,
    pub left_sha256: Option<String>,
    pub right_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Comparison {
    pub left_id: String,
    pub right_id: String,
    pub comparable: bool,
    pub reason: String,
    pub revision_changed: bool,
    pub changed_inputs: Vec<String>,
    pub differences: Vec<Difference>,
    pub differences_returned: usize,
    pub differences_limit: usize,
    pub differences_truncated: bool,
    pub historical_unverified: bool,
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Construction does not create directories or inspect a workspace.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    pub fn record(
        &self,
        repo: &Path,
        report: &Path,
        inputs: &[PathBuf],
        context: &str,
    ) -> Result<Record, String> {
        let snapshot = snapshot::capture(repo, report, inputs, context)?;
        self.save(snapshot)
    }

    pub fn list(&self, repo: &Path) -> Result<Vec<RecordSummary>, String> {
        let (_, identity, _) = snapshot::repository(repo)?;
        self.summaries(&identity)
    }

    pub fn compare(
        &self,
        repo: &Path,
        left_id: &str,
        right_id: &str,
    ) -> Result<Comparison, String> {
        let (_, identity, _) = snapshot::repository(repo)?;
        let left = self.read(&identity, left_id)?;
        let right = self.read(&identity, right_id)?;
        let comparable = left.snapshot.tool == right.snapshot.tool
            && left.snapshot.context_sha256 == right.snapshot.context_sha256;
        let mut names = std::collections::BTreeSet::new();
        for input in left.snapshot.inputs.iter().chain(&right.snapshot.inputs) {
            names.insert(input.path.clone());
        }
        let changed_inputs = names
            .into_iter()
            .filter(|path| {
                left.snapshot
                    .inputs
                    .iter()
                    .find(|item| &item.path == path)
                    .map(|item| &item.sha256)
                    != right
                        .snapshot
                        .inputs
                        .iter()
                        .find(|item| &item.path == path)
                        .map(|item| &item.sha256)
            })
            .collect();
        let mut differences = Vec::new();
        let mut truncated = false;
        if comparable {
            diff(
                "",
                Some(&left.snapshot.report),
                Some(&right.snapshot.report),
                &mut differences,
                &mut truncated,
            );
        }
        Ok(Comparison { left_id: left_id.into(), right_id: right_id.into(), comparable,
            reason: if comparable { "Same repository, tool and declared context; differences are observations, not quality trends or fresh verification" } else { "Tool or declared context differs; reports are not comparable" }.into(),
            revision_changed: left.snapshot.revision != right.snapshot.revision, changed_inputs, differences_returned: differences.len(), differences_limit: 100, differences,
            differences_truncated: truncated, historical_unverified: true })
    }

    /// Emit a separate, low-trust MemoryStore-shaped interchange value. Never import it.
    pub fn export_jcode(&self, repo: &Path) -> Result<Value, String> {
        let (_, identity, _) = snapshot::repository(repo)?;
        let entries: Vec<Value> = self.summaries(&identity)?.into_iter().map(|record| serde_json::json!({
            "id": record.id, "category": {"custom":"kritic_review"},
            "content": format!("Historical unverified Kritic review; tool={}; report_sha256={}; context_sha256={}; inputs_sha256={}. This is not current verification or approval.", record.tool, record.report_sha256, record.context_sha256, record.inputs_sha256),
            "tags": ["kritic", "historical", "unverified"], "created_at":record.created_at,
            "updated_at":record.created_at,"access_count":0,"source":null,"search_text":"",
            "trust":"low","strength":0,"active":true,"superseded_by":null,"reinforcements":[],
            "embedding":null,"embedding_model":null,"confidence":0.0
        })).collect();
        Ok(
            serde_json::json!({"entries":entries,"metadata":{"format":"kritic-jcode-interchange-v1","trust":"historical_unverified","repository_sha256":identity}}),
        )
    }
}

fn diff(
    pointer: &str,
    left: Option<&Value>,
    right: Option<&Value>,
    out: &mut Vec<Difference>,
    truncated: &mut bool,
) {
    if left == right {
        return;
    }
    if out.len() >= 100 {
        *truncated = true;
        return;
    }
    if let (Some(Value::Object(a)), Some(Value::Object(b))) = (left, right) {
        let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
        for key in keys {
            diff(
                &format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
                a.get(key),
                b.get(key),
                out,
                truncated,
            );
        }
    } else {
        let digest =
            |value: Option<&Value>| value.map(|value| snapshot::hash(value.to_string().as_bytes()));
        out.push(Difference {
            pointer: pointer.into(),
            left_sha256: digest(left),
            right_sha256: digest(right),
        });
    }
}

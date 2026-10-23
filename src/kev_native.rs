//! Upstream jaredpalmer/kev protocol (api.py / serve.py, 2074cc44).
//! Model aliases are echoed, so identity comes from /v1/models, never the answer's model.
use crate::{
    config::Config,
    kev::{Question, parse_answer},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, Serialize)]
pub struct Models {
    pub models: Vec<Model>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Model {
    pub name: String,
    pub run: String,
    pub base: String,
    pub backend: String,
    pub device: String,
    pub dtype: String,
}
impl Models {
    pub fn validate(&self, cfg: &Config) -> Result<(), String> {
        if self.models.is_empty() || !self.models.iter().any(|m| m.name == cfg.model) {
            return Err("native MLX model alias unavailable".into());
        }
        if self.models.iter().any(|m| {
            m.run != cfg.native_run
                || m.base != cfg.native_base
                || m.backend != "mlx"
                || m.device != "mps"
                || m.dtype != cfg.native_dtype
        }) {
            return Err(
                "native MLX runtime identity mismatch (run/base/backend/device/dtype); no fallback"
                    .into(),
            );
        }
        Ok(())
    }
}
#[derive(Deserialize)]
struct Response {
    model: String,
    answers: BTreeMap<String, NativeAnswer>,
    usage: Usage,
    #[serde(default)]
    truncated: bool,
}
#[derive(Deserialize, Serialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "type")]
enum NativeAnswer {
    #[serde(rename = "noul")]
    Noul { noul: f64 },
    #[serde(rename = "choice")]
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    #[serde(rename = "score")]
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        legend: BTreeMap<String, String>,
        confidence: f64,
    },
}
pub fn normalize(
    value: Value,
    questions: &BTreeMap<String, Question>,
    model: &str,
) -> Result<Value, String> {
    let response: Response =
        serde_json::from_value(value).map_err(|e| format!("native MLX malformed response: {e}"))?;
    if response.model != model
        || response.answers.len() != questions.len()
        || questions.keys().any(|k| !response.answers.contains_key(k))
    {
        return Err("native MLX response model/question IDs mismatch".into());
    }
    let mut answers = serde_json::Map::new();
    for (id, answer) in response.answers {
        let q = &questions[&id];
        let mut raw = serde_json::to_value(&answer).map_err(|e| e.to_string())?;
        match &answer {
            // Upstream noul has no confidence. Derive certainty from its binary probability,
            // never a calibrated reliability or acceptance score.
            NativeAnswer::Noul { noul } => raw["confidence"] = json!(noul.max(1.0 - noul)),
            NativeAnswer::Score {
                probabilities,
                legend,
                ..
            } => {
                let levels = q
                    .criteria
                    .as_ref()
                    .and_then(Value::as_array)
                    .ok_or("native score question lacks levels")?;
                if probabilities.len() != levels.len()
                    || legend.len() != levels.len()
                    || (0..levels.len()).any(|i| {
                        !probabilities.contains_key(&i.to_string())
                            || !legend.contains_key(&i.to_string())
                    })
                {
                    return Err("native score distribution/legend must contain exactly the requested level indices".into());
                }
            }
            _ => {}
        }
        parse_answer(&id, q, &raw)?;
        answers.insert(id, raw);
    }
    Ok(json!({"answers": answers, "usage": response.usage, "truncated": response.truncated}))
}

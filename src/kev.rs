//! Kev-4B client over laya's TypeSafe System One API (`POST /v1/decide`).
//!
//! Kev returns calibrated probabilities for typed questions (noul / choice / score). It never
//! generates text. Every answer is validated against the question that was asked; anything
//! missing or out of range is an error, never a default.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::config::Config;

/// laya's wire format for one question. Built only by `crate::spec::DecisionSpec::wire`, so
/// every question Kev sees names its fields and defines its answers (see src/spec.rs).
#[derive(Debug, Clone, Serialize)]
pub struct Question {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria: Option<Value>,
}

impl Question {
    fn levels(&self) -> usize {
        match &self.criteria {
            Some(Value::Array(a)) => a.len(),
            Some(Value::Object(o)) => o.len(),
            _ => 0,
        }
    }
    fn est_chars(&self) -> usize {
        self.instructions.len() + self.criteria.as_ref().map(|c| c.to_string().len()).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Answer {
    /// noul: probability of "yes"; score: expected level (0..levels-1); choice: probability of `choice`.
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probabilities: Option<BTreeMap<String, f64>>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Decision {
    pub answers: BTreeMap<String, Answer>,
    /// True when laya reported a full-length row: the state was cut to fit and Kev did not
    /// see all of it.
    pub truncated: bool,
    pub latency_ms: u64,
    pub input_tokens: u64,
}

/// Anything that can answer typed questions about a state. The real implementation is
/// `KevClient`; tests use a fake.
pub trait Decider: Send + Sync {
    fn decide(
        &self,
        state: &Value,
        questions: &BTreeMap<String, Question>,
    ) -> impl std::future::Future<Output = Result<Decision, String>> + Send;
}

pub struct KevClient {
    pub cfg: Arc<Config>,
    http: reqwest::Client,
    start_lock: tokio::sync::Mutex<()>,
}

/// Conservative token estimate for one row (state + one question). Measured on laya: the
/// JSON-serialized state costs ~1.15 chars/token for code (escaping + Qwen3.5 tokenizer).
pub fn est_row_tokens(state: &Value, q: &Question) -> usize {
    (state.to_string().len() + q.est_chars()) * 10 / 11 + 48
}

/// Split questions into groups so rows x tokens per request stays within `budget`.
pub fn group_questions(
    state: &Value,
    questions: &BTreeMap<String, Question>,
    budget: usize,
) -> Vec<BTreeMap<String, Question>> {
    let mut groups: Vec<BTreeMap<String, Question>> = Vec::new();
    let mut cur = BTreeMap::new();
    let mut used = 0usize;
    for (id, q) in questions {
        let t = est_row_tokens(state, q);
        if !cur.is_empty() && used + t > budget {
            groups.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.insert(id.clone(), q.clone());
        used += t;
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    groups
}

/// Validate one laya answer against the question asked.
pub fn parse_answer(id: &str, q: &Question, raw: &Value) -> Result<Answer, String> {
    let bad = |why: &str| format!("kev answer `{id}` invalid: {why}");
    let ty = raw.get("type").and_then(Value::as_str).ok_or_else(|| bad("missing type"))?;
    if ty != q.kind {
        return Err(bad(&format!("type {ty} != asked {}", q.kind)));
    }
    let confidence = raw.get("confidence").and_then(Value::as_f64).ok_or_else(|| bad("missing confidence"))?;
    let probs = raw.get("probabilities").and_then(Value::as_object).map(|o| {
        o.iter().filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f))).collect::<BTreeMap<_, _>>()
    });
    let in01 = |v: f64| (0.0..=1.0).contains(&v);
    match q.kind {
        "noul" => {
            let v = raw.get("noul").and_then(Value::as_f64).ok_or_else(|| bad("missing noul"))?;
            if !in01(v) {
                return Err(bad("noul out of [0,1]"));
            }
            Ok(Answer { value: v, choice: None, probabilities: None, confidence })
        }
        "choice" => {
            let c = raw.get("choice").and_then(Value::as_str).ok_or_else(|| bad("missing choice"))?;
            let probs = probs.ok_or_else(|| bad("missing probabilities"))?;
            let asked = q.criteria.as_ref().and_then(Value::as_object).ok_or_else(|| bad("question has no options"))?;
            if !asked.contains_key(c) {
                return Err(bad(&format!("choice `{c}` was not an option")));
            }
            let p = *probs.get(c).ok_or_else(|| bad("chosen option has no probability"))?;
            Ok(Answer { value: p, choice: Some(c.to_string()), probabilities: Some(probs), confidence })
        }
        "score" => {
            let v = raw.get("score").and_then(Value::as_f64).ok_or_else(|| bad("missing score"))?;
            if v < 0.0 || v > (q.levels().saturating_sub(1)) as f64 + 1e-6 {
                return Err(bad("score out of range"));
            }
            Ok(Answer { value: v, choice: None, probabilities: probs, confidence })
        }
        other => Err(bad(&format!("unknown type {other}"))),
    }
}

impl KevClient {
    pub fn new(cfg: Arc<Config>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(cfg.kev_request_s))
            .build()
            .expect("reqwest client");
        KevClient { cfg, http, start_lock: tokio::sync::Mutex::new(()) }
    }

    pub async fn health(&self) -> Option<Value> {
        let r = self.http.get(format!("{}/health", self.cfg.laya_url)).timeout(Duration::from_secs(3)).send().await.ok()?;
        if !r.status().is_success() {
            return None;
        }
        r.json::<Value>().await.ok()
    }

    /// Start laya if it isn't answering. Returns Err with a human-readable reason on failure.
    pub async fn ensure_up(&self) -> Result<(), String> {
        if self.health().await.is_some() {
            crate::events::ready("laya", "up", "");
            return Ok(());
        }
        let _guard = self.start_lock.lock().await;
        if self.health().await.is_some() {
            crate::events::ready("laya", "up", "");
            return Ok(());
        }
        let r = self.start_laya().await;
        match &r {
            Ok(()) => crate::events::ready("laya", "up", "started"),
            Err(e) => crate::events::ready("laya", "failed", e),
        }
        r
    }

    async fn start_laya(&self) -> Result<(), String> {
        crate::events::ready("laya", "starting", "loading Kev-4B");
        let exe = &self.cfg.laya_exe;
        if !exe.exists() {
            return Err(format!("laya unreachable at {} and {} not found (set KRITIC_LAYA_EXE)", self.cfg.laya_url, exe.display()));
        }
        if !self.cfg.kev_gguf.exists() {
            return Err(format!("Kev GGUF not found at {} (set KRITIC_KEV_GGUF)", self.cfg.kev_gguf.display()));
        }
        let _ = std::fs::create_dir_all(&self.cfg.cuda_cache);
        let log = self.cfg.log_path.with_file_name("laya.log");
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let out = std::fs::File::create(&log).map_err(|e| format!("cannot create {}: {e}", log.display()))?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["serve"])
            .arg(&self.cfg.kev_gguf)
            .args(["--port", &self.cfg.laya_port.to_string(), "--device", "auto", "--cuda-graph"])
            .current_dir(exe.parent().unwrap_or(std::path::Path::new(".")))
            // Persist the sm_89 PTX -> sm_120 JIT so restarts skip the ~40s compile.
            .env("CUDA_CACHE_PATH", &self.cfg.cuda_cache)
            .env("CUDA_CACHE_MAXSIZE", "4294967296")
            .stdin(std::process::Stdio::null())
            .stdout(out)
            .stderr(err);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }
        cmd.spawn().map_err(|e| format!("failed to start laya: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if self.health().await.is_some() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(format!("started laya but it did not answer within 120s (see {})", log.display()))
    }

    /// One Kev request with a hard deadline. reqwest's own timeout once failed to end a request laya
    /// received and never answered (a critic call hung for over an hour), so the whole exchange is
    /// also bounded here. A stalled laya is stopped: its health check still answers while a
    /// request is stuck, so leaving it running would stall the next call too. The next call
    /// starts a fresh one (~13 s with the CUDA cache).
    async fn post(&self, state: &Value, questions: &BTreeMap<String, Question>) -> Result<(Value, u64), String> {
        let limit = Duration::from_secs(self.cfg.kev_request_s);
        match tokio::time::timeout(limit, self.exchange(state, questions)).await {
            Ok(result) => result,
            Err(_) => {
                crate::events::ready("laya", "stalled", &format!("no answer in {}s; restarting it", limit.as_secs()));
                crate::gpu::stop_laya(&self.cfg).await;
                Err(format!("kev request stalled: laya did not answer within {}s (stopped it; the next call restarts it)", limit.as_secs()))
            }
        }
    }

    async fn exchange(&self, state: &Value, questions: &BTreeMap<String, Question>) -> Result<(Value, u64), String> {
        let body = json!({"model": self.cfg.model, "state": state, "questions": questions});
        let t0 = Instant::now();
        let r = self
            .http
            .post(format!("{}/v1/decide", self.cfg.laya_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| if e.is_timeout() { format!("kev request timed out ({}s)", self.cfg.kev_request_s) } else { format!("laya request failed: {e}") })?;
        let status = r.status();
        let v: Value = r.json().await.map_err(|e| format!("laya returned non-JSON ({status}): {e}"))?;
        if !status.is_success() {
            let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown error");
            return Err(format!("laya HTTP {status}: {msg}"));
        }
        Ok((v, t0.elapsed().as_millis() as u64))
    }
}

impl Decider for KevClient {
    async fn decide(&self, state: &Value, questions: &BTreeMap<String, Question>) -> Result<Decision, String> {
        self.ensure_up().await?;
        let mut out = Decision::default();
        for group in group_questions(state, questions, self.cfg.token_budget) {
            let (v, ms) = self.post(state, &group).await?;
            out.latency_ms += ms;
            let tokens = v.pointer("/usage/input_tokens").and_then(Value::as_u64).unwrap_or(0);
            out.input_tokens += tokens;
            crate::usage::log_model_usage(
                &self.cfg,
                json!({"model": self.cfg.model, "input_tokens": tokens, "output_tokens": 0, "latency_ms": ms, "rows": group.len()}),
            );
            let answers = v.get("answers").and_then(Value::as_object).ok_or("laya response has no answers")?;
            for (id, q) in &group {
                let raw = answers.get(id).ok_or_else(|| format!("kev did not answer `{id}`"))?;
                out.answers.insert(id.clone(), parse_answer(id, q, raw)?);
            }
            let tokens = v.pointer("/usage/input_tokens").and_then(Value::as_u64).unwrap_or(0) as usize;
            if tokens / group.len().max(1) >= self.cfg.max_seq_tokens.saturating_sub(8) {
                out.truncated = true;
            }
        }
        Ok(out)
    }
}

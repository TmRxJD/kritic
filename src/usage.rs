//! Token and context metrics.
//!
//! Claude side: parsed from Claude Code's session transcript (`~/.claude/projects/<proj>/<session>.jsonl`),
//! which records per-API-call usage (fresh input, cache read, cache write, output, thinking) and
//! timestamps. Context occupancy of a call = input + cache_read + cache_creation.
//!
//! Local side: every Kev (laya) and vision (Ollama) request appends a line to
//! `logs/model_usage.jsonl` with its token counts and timings.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::Config;

// ------------------------------------------------------------------ local model usage log

pub fn model_usage_path(cfg: &Config) -> PathBuf {
    cfg.log_path.with_file_name("model_usage.jsonl")
}

/// Append one local-model request record. Never fails the caller.
pub fn log_model_usage(cfg: &Config, rec: Value) {
    let path = model_usage_path(cfg);
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if path
        .metadata()
        .map(|m| m.len() > 5_000_000)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, path.with_extension("old.jsonl"));
    }
    let mut rec = rec;
    rec["ts"] = json!(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{rec}");
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct LocalModelStats {
    pub model: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_ms: u64,
    /// input tokens processed per second (Kev has no output tokens: it scores options).
    pub input_tokens_per_s: Option<f64>,
    /// generated tokens per second (vision), from Ollama's eval timings.
    pub output_tokens_per_s: Option<f64>,
    pub load_ms_total: u64,
}

pub fn local_model_stats(cfg: &Config, since: Option<DateTime<Utc>>) -> Vec<LocalModelStats> {
    let Ok(f) = std::fs::File::open(model_usage_path(cfg)) else {
        return Vec::new();
    };
    let mut by: BTreeMap<String, (LocalModelStats, u64)> = BTreeMap::new();
    for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let (Some(s), Some(ts)) = (
            since,
            v["ts"]
                .as_str()
                .and_then(|t| t.parse::<DateTime<Utc>>().ok()),
        ) && ts < s
        {
            continue;
        }
        let model = v["model"].as_str().unwrap_or("?").to_string();
        let e = by.entry(model.clone()).or_insert_with(|| {
            (
                LocalModelStats {
                    model,
                    ..Default::default()
                },
                0,
            )
        });
        e.0.requests += 1;
        e.0.input_tokens += v["input_tokens"].as_u64().unwrap_or(0);
        e.0.output_tokens += v["output_tokens"].as_u64().unwrap_or(0);
        e.0.total_ms += v["latency_ms"].as_u64().unwrap_or(0);
        e.0.load_ms_total += v["load_ms"].as_u64().unwrap_or(0);
        e.1 += v["eval_ms"].as_u64().unwrap_or(0);
    }
    by.into_values()
        .map(|(mut s, eval_ms)| {
            let compute_ms = s.total_ms.saturating_sub(s.load_ms_total);
            if compute_ms > 0 && s.input_tokens > 0 {
                s.input_tokens_per_s =
                    Some(round1(s.input_tokens as f64 * 1000.0 / compute_ms as f64));
            }
            if eval_ms > 0 && s.output_tokens > 0 {
                s.output_tokens_per_s =
                    Some(round1(s.output_tokens as f64 * 1000.0 / eval_ms as f64));
            }
            s
        })
        .collect()
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

// ------------------------------------------------------------------ Claude transcript

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct CallUsage {
    pub ts: String,
    pub model: String,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    /// input + cache_read + cache_write: the context window occupied by this request.
    pub context_tokens: u64,
    /// Time from the previous transcript entry to this response being recorded (includes
    /// time-to-first-token); an approximation of the call's duration.
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct TaskUsage {
    pub index: usize,
    pub prompt: String,
    pub started: String,
    pub wall_s: f64,
    pub api_calls: usize,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    /// Sum of context_tokens over the task's calls: what the task actually cost in processed input.
    pub processed_input_tokens: u64,
    pub context_start: u64,
    pub context_end: u64,
    pub context_peak: u64,
    pub output_tokens_per_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct SessionUsage {
    pub session_id: String,
    pub transcript: String,
    pub model: String,
    pub api_calls: usize,
    pub tasks: usize,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub processed_input_tokens: u64,
    pub context_now: u64,
    pub context_peak: u64,
    pub context_limit: u64,
    pub context_utilization_pct: f64,
    /// Average context growth per API call over the last 20 calls.
    pub context_growth_per_call: f64,
    pub compactions: usize,
    pub cache_hit_rate_pct: f64,
    pub output_tokens_per_s_median: Option<f64>,
    pub output_tokens_per_task_avg: f64,
    pub processed_input_per_task_avg: f64,
}

pub fn projects_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PathBuf::from(home).join(".claude").join("projects")
}

/// Most recently modified transcript, optionally for one session id.
pub fn find_transcript(session_id: Option<&str>) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for proj in std::fs::read_dir(projects_dir()).ok()?.flatten() {
        let Ok(rd) = std::fs::read_dir(proj.path()) else {
            continue;
        };
        for f in rd.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(id) = session_id {
                if p.file_stem().and_then(|s| s.to_str()) == Some(id) {
                    return Some(p);
                }
                continue;
            }
            let m = f.metadata().and_then(|m| m.modified()).ok()?;
            if best.as_ref().is_none_or(|(t, _)| m > *t) {
                best = Some((m, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Parse a transcript into per-call usage and per-task aggregates.
pub fn analyze(
    path: &Path,
    context_limit: u64,
) -> Result<(SessionUsage, Vec<TaskUsage>, Vec<CallUsage>), String> {
    let f =
        std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut calls: Vec<(usize, CallUsage)> = Vec::new(); // (task index, call)
    let mut tasks: Vec<TaskUsage> = Vec::new();
    // message id -> (index in `calls`, request start). A response is written as one entry per
    // content block; the call ends at its last block.
    let mut seen_ids: std::collections::HashMap<String, (usize, Option<DateTime<Utc>>)> =
        std::collections::HashMap::new();
    let mut prev_ts: Option<DateTime<Utc>> = None;
    let mut compactions = 0;
    let mut model = String::new();
    for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let ts = v["timestamp"]
            .as_str()
            .and_then(|t| t.parse::<DateTime<Utc>>().ok());
        let kind = v["type"].as_str().unwrap_or("");
        if v["isCompactSummary"] == true
            || (kind == "system" && v["subtype"].as_str().is_some_and(|s| s.contains("compact")))
        {
            compactions += 1;
        }
        if kind == "user" && v["origin"]["kind"] == "human" {
            let prompt: String = text_of(&v["message"]["content"])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(100)
                .collect();
            tasks.push(TaskUsage {
                index: tasks.len(),
                prompt,
                started: v["timestamp"].as_str().unwrap_or("").into(),
                ..Default::default()
            });
        }
        if kind == "assistant" && v["isSidechain"] != true {
            let m = &v["message"];
            let id = m["id"].as_str().unwrap_or("").to_string();
            let u = &m["usage"];
            let g = |k: &str| u[k].as_u64().unwrap_or(0);
            let ctx =
                g("input_tokens") + g("cache_read_input_tokens") + g("cache_creation_input_tokens");
            if let Some((idx, Some(start))) = seen_ids.get(&id).copied()
                && let Some(end) = ts
                && end > start
            {
                calls[idx].1.duration_ms = Some((end - start).num_milliseconds() as u64);
            }
            if !id.is_empty() && ctx > 0 && !seen_ids.contains_key(&id) {
                seen_ids.insert(id.clone(), (calls.len(), prev_ts));
                if let Some(mm) = m["model"].as_str() {
                    model = mm.to_string();
                }
                let duration_ms = match (prev_ts, ts) {
                    (Some(a), Some(b)) if b > a => Some((b - a).num_milliseconds() as u64),
                    _ => None,
                };
                calls.push((
                    tasks.len().saturating_sub(1),
                    CallUsage {
                        ts: v["timestamp"].as_str().unwrap_or("").into(),
                        model: m["model"].as_str().unwrap_or("").into(),
                        input_tokens: g("input_tokens"),
                        cache_read_tokens: g("cache_read_input_tokens"),
                        cache_write_tokens: g("cache_creation_input_tokens"),
                        output_tokens: g("output_tokens"),
                        thinking_tokens: u["output_tokens_details"]["thinking_tokens"]
                            .as_u64()
                            .unwrap_or(0),
                        context_tokens: ctx,
                        duration_ms,
                    },
                ));
            }
        }
        if ts.is_some() {
            prev_ts = ts;
        }
    }
    if tasks.is_empty() {
        tasks.push(TaskUsage {
            prompt: "(no human prompt recorded)".into(),
            ..Default::default()
        });
    }
    let mut rates = Vec::new();
    for (ti, c) in &calls {
        let t = &mut tasks[*ti];
        if t.api_calls == 0 {
            t.context_start = c.context_tokens;
        }
        t.api_calls += 1;
        t.input_tokens += c.input_tokens;
        t.cache_read_tokens += c.cache_read_tokens;
        t.cache_write_tokens += c.cache_write_tokens;
        t.output_tokens += c.output_tokens;
        t.thinking_tokens += c.thinking_tokens;
        t.processed_input_tokens += c.context_tokens;
        t.context_end = c.context_tokens;
        t.context_peak = t.context_peak.max(c.context_tokens);
        if let Some(d) = c.duration_ms.filter(|d| *d > 200) {
            rates.push(c.output_tokens as f64 * 1000.0 / d as f64);
        }
    }
    for t in tasks.iter_mut() {
        let tc: Vec<&CallUsage> = calls
            .iter()
            .filter(|(i, _)| *i == t.index)
            .map(|(_, c)| c)
            .collect();
        let dur: u64 = tc.iter().filter_map(|c| c.duration_ms).sum();
        if dur > 0 {
            t.output_tokens_per_s = Some(round1(t.output_tokens as f64 * 1000.0 / dur as f64));
        }
        if let (Some(a), Some(b)) = (
            t.started.parse::<DateTime<Utc>>().ok(),
            tc.last().and_then(|c| c.ts.parse::<DateTime<Utc>>().ok()),
        ) {
            t.wall_s = round1((b - a).num_milliseconds().max(0) as f64 / 1000.0);
        }
    }
    rates.sort_by(|a, b| a.total_cmp(b));
    let only: Vec<&CallUsage> = calls.iter().map(|(_, c)| c).collect();
    let sum = |f: fn(&CallUsage) -> u64| only.iter().map(|c| f(c)).sum::<u64>();
    let n = only.len();
    let last20 = &only[n.saturating_sub(20)..];
    let growth = if last20.len() > 1 {
        (last20.last().unwrap().context_tokens as f64 - last20[0].context_tokens as f64)
            / (last20.len() - 1) as f64
    } else {
        0.0
    };
    let context_now = only.last().map(|c| c.context_tokens).unwrap_or(0);
    let context_peak = only.iter().map(|c| c.context_tokens).max().unwrap_or(0);
    let processed = sum(|c| c.context_tokens);
    let task_count = tasks.iter().filter(|t| t.api_calls > 0).count().max(1);
    let s = SessionUsage {
        session_id: path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .into(),
        transcript: path.display().to_string(),
        model,
        api_calls: n,
        tasks: tasks.len(),
        input_tokens: sum(|c| c.input_tokens),
        cache_read_tokens: sum(|c| c.cache_read_tokens),
        cache_write_tokens: sum(|c| c.cache_write_tokens),
        output_tokens: sum(|c| c.output_tokens),
        thinking_tokens: sum(|c| c.thinking_tokens),
        processed_input_tokens: processed,
        context_now,
        context_peak,
        context_limit,
        context_utilization_pct: round1(context_now as f64 * 100.0 / context_limit.max(1) as f64),
        context_growth_per_call: round1(growth),
        compactions,
        cache_hit_rate_pct: round1(
            sum(|c| c.cache_read_tokens) as f64 * 100.0 / processed.max(1) as f64,
        ),
        output_tokens_per_s_median: rates.get(rates.len() / 2).map(|r| round1(*r)),
        output_tokens_per_task_avg: round1(sum(|c| c.output_tokens) as f64 / task_count as f64),
        processed_input_per_task_avg: round1(processed as f64 / task_count as f64),
    };
    Ok((s, tasks, only.into_iter().cloned().collect()))
}

// ------------------------------------------------------------------ Hermes

/// Context use of one Hermes session, from the kritic context-engine plugin's usage log
/// (`<home>/logs/hermes-usage.jsonl`, one line per model call).
#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct HermesUsage {
    pub session_id: String,
    pub api_calls: usize,
    /// Prompt tokens of the latest call: the context as the model last saw it.
    pub context_now: u64,
    pub context_peak: u64,
    pub context_length: u64,
    pub context_utilization_pct: f64,
    /// Mean change in prompt tokens per call over the last 20 calls.
    pub context_growth_per_call: f64,
    /// Compactions the kritic engine has run, and messages it has archived (recallable).
    pub compactions: u64,
    pub archived_messages: u64,
    pub processed_input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
}

/// The given Hermes session, or the one with the latest call.
pub fn hermes_usage(cfg: &Config, session: Option<&str>) -> Option<HermesUsage> {
    hermes_usage_from(&cfg.log_path.with_file_name("hermes-usage.jsonl"), session)
}

pub fn hermes_usage_from(path: &Path, session: Option<&str>) -> Option<HermesUsage> {
    let rows: Vec<Value> = std::fs::read_to_string(path)
        .ok()?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let sid = match session {
        Some(s) => s.to_string(),
        None => rows.last()?["session_id"].as_str()?.to_string(),
    };
    let rows: Vec<&Value> = rows
        .iter()
        .filter(|r| r["session_id"] == sid.as_str())
        .collect();
    if rows.is_empty() {
        return None;
    }
    let n = |r: &Value, k: &str| r[k].as_u64().unwrap_or(0);
    let prompts: Vec<u64> = rows.iter().map(|r| n(r, "prompt_tokens")).collect();
    let last = rows.last()?;
    let window = &prompts[prompts.len().saturating_sub(21)..];
    let growth = if window.len() > 1 {
        (window[window.len() - 1] as f64 - window[0] as f64) / (window.len() - 1) as f64
    } else {
        0.0
    };
    let length = n(last, "context_length");
    let now = *prompts.last()?;
    Some(HermesUsage {
        session_id: sid,
        api_calls: rows.len(),
        context_now: now,
        context_peak: prompts.iter().copied().max().unwrap_or(0),
        context_length: length,
        context_utilization_pct: if length > 0 {
            (now as f64 / length as f64 * 1000.0).round() / 10.0
        } else {
            0.0
        },
        context_growth_per_call: (growth * 10.0).round() / 10.0,
        compactions: n(last, "compression_count"),
        archived_messages: n(last, "archived_total"),
        processed_input_tokens: prompts.iter().sum(),
        cache_read_tokens: rows.iter().map(|r| n(r, "cache_read_tokens")).sum(),
        output_tokens: rows.iter().map(|r| n(r, "completion_tokens")).sum(),
    })
}

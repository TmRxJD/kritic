//! kev-tools: stdio MCP server shared by Claude Code and Codex.
//!
//! Tools: `trim`, `digest` (extractive, ranked by local Kev; no generation model),
//! `pick_tier` (advisory), `handoff_latest`, `handoff_write`.

mod search;
mod staleness;
mod worktrees;

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const KEV_URL: &str = "http://127.0.0.1:8089/v1/decide";
const HANDOFF_DIR: &str = "E:/AI/claude-handoffs";
const KEV_BUDGET: Duration = Duration::from_secs(20);
const MAX_KEV_CALLS: usize = 40;
const ANCHOR: usize = 600;
const MAX_FILE_BYTES: u64 = 20_000_000;

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let Some(id) = req.get("id").cloned() else {
            continue;
        }; // notification: no reply
        let resp = match req["method"].as_str().unwrap_or("") {
            "initialize" => ok(
                id,
                json!({
                    "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "kev-tools", "version": env!("CARGO_PKG_VERSION")},
                }),
            ),
            "ping" => ok(id, json!({})),
            "tools/list" => ok(id, json!({"tools": tools()})),
            "tools/call" => {
                let name = req["params"]["name"].as_str().unwrap_or("");
                let (text, is_error) = call(name, &req["params"]["arguments"]);
                ok(
                    id,
                    json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
                )
            }
            _ => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}})
            }
        };
        let _ = writeln!(out, "{resp}");
        let _ = out.flush();
    }
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn tools() -> Value {
    json!([
        search::tool(),
        staleness::tool(),
        worktrees::tool(),
        {
            "name": "trim",
            "description": "Shrink long text to what matters for `query`: keeps the chunks local Kev judges relevant (plus first/last 600 chars), drops the rest with a marker. Extractive only. Falls back to head+tail if Kev is down.",
            "inputSchema": {"type": "object", "properties": {
                "query": {"type": "string", "description": "The current task/question."},
                "text": {"type": "string"},
                "path": {"type": "string", "description": "Absolute file path to read instead of `text`."},
                "budget_chars": {"type": "number", "description": "Default 6000."},
                "min_relevance": {"type": "number", "description": "Default 0.3."}
            }, "required": ["query"]}
        },
        {
            "name": "digest",
            "description": "Search large files for what answers `question` without reading them into context: returns only the top Kev-ranked excerpts with path:line, extractive (no generation model). Use for big logs, dumps, long source files.",
            "inputSchema": {"type": "object", "properties": {
                "question": {"type": "string"},
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Absolute file paths."},
                "budget_chars": {"type": "number", "description": "Default 6000."},
                "min_relevance": {"type": "number", "description": "Default 0.3."}
            }, "required": ["question", "paths"]}
        },
        {
            "name": "pick_tier",
            "description": "Advisory: asks local Kev whether `task` is mechanical (small model) or needs reasoning (large model). Returns {tier, by}.",
            "inputSchema": {"type": "object", "properties": {"task": {"type": "string"}}, "required": ["task"]}
        },
        {
            "name": "handoff_latest",
            "description": "Returns the newest session handoff brief from E:/AI/claude-handoffs (shared by Claude Code and Codex). Call ONLY when the user explicitly asks to continue from a handoff; never proactively.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "handoff_write",
            "description": "Writes a handoff brief (markdown: Goal, Current state, Key decisions, Files, Next steps, Open questions) to E:/AI/claude-handoffs/<UTC timestamp>.md and returns the path. Call ONLY when the user explicitly asks for a handoff.",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}
        }
    ])
}

fn call(name: &str, a: &Value) -> (String, bool) {
    let r = match name {
        "search_queries" => search::call(a),
        "staleness_evaluate" => staleness::call(a),
        "worktree_evaluate" => worktrees::call(a),
        "trim" => trim(a),
        "digest" => digest(a),
        "pick_tier" => pick_tier(a),
        "handoff_latest" => handoff_latest(),
        "handoff_write" => handoff_write(a),
        _ => Err(format!("unknown tool {name}")),
    };
    match r {
        Ok(t) => (t, false),
        Err(e) => (format!("kev-tools {name}: {e}"), true),
    }
}

// ---------- Kev ----------

fn kev_post(state: Value, key: &str, question: Value) -> Result<Value, String> {
    Ok(kev_questions(state, json!({key: question}))?[key].clone())
}

fn kev_questions(state: Value, questions: Value) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let body = json!({"model": "kev-4b", "state": state, "questions": questions});
    let mut resp = agent
        .post(KEV_URL)
        .send_json(&body)
        .map_err(|e| format!("kev unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("kev HTTP {}", resp.status()));
    }
    let v: Value = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("kev bad JSON: {e}"))?;
    v.get("answers")
        .filter(|a| a.is_object())
        .cloned()
        .ok_or_else(|| "kev answer missing answers object".to_string())
}

fn kev_relevance(query: &str, chunk: &str) -> Result<f64, String> {
    let q = json!({
        "type": "noul",
        "instructions": "Does `chunk` contain information needed to answer `query`?",
        "criteria": {
            "true": "a fact, definition or line in `chunk` is needed to answer `query`",
            "false": "nothing in `chunk` is needed to answer `query`"
        }
    });
    let state = json!({"query": clip(query, 1000), "chunk": clip(chunk, 1400)});
    kev_post(state, "relevant", q)?["noul"]
        .as_f64()
        .ok_or_else(|| "kev answer missing noul".to_string())
}

fn clip(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

// ---------- chunking ----------

struct Chunk {
    src: String,
    line: usize,
    text: String,
}

fn chunk_text(src: &str, text: &str, size: usize) -> Vec<Chunk> {
    let mut out = Vec::new();
    let (mut cur, mut start) = (String::new(), 1usize);
    for (i, line) in text.lines().enumerate() {
        if !cur.is_empty() && cur.len() + line.len() + 1 > size {
            out.push(Chunk {
                src: src.to_string(),
                line: start,
                text: std::mem::take(&mut cur),
            });
            start = i + 1;
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.is_empty() {
        out.push(Chunk {
            src: src.to_string(),
            line: start,
            text: cur,
        });
    }
    out
}

fn lexical(query: &str, text: &str) -> usize {
    let low = text.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() >= 3 && seen.insert(w.to_string()))
        .filter(|w| low.contains(w))
        .count()
}

/// Kev scores per chunk (None = unscored). Returns Err if Kev is unreachable at the first call.
fn score(query: &str, chunks: &[&Chunk]) -> Result<Vec<Option<f64>>, String> {
    let deadline = Instant::now() + KEV_BUDGET;
    let mut out = vec![None; chunks.len()];
    for (i, c) in chunks.iter().enumerate() {
        if Instant::now() > deadline {
            break;
        }
        match kev_relevance(query, &c.text) {
            Ok(s) => out[i] = Some(s),
            Err(e) if i == 0 => return Err(e),
            Err(_) => break,
        }
    }
    Ok(out)
}

// ---------- tools ----------

fn num(a: &Value, k: &str, d: f64) -> f64 {
    a[k].as_f64().unwrap_or(d)
}

fn read_file(p: &str) -> Result<String, String> {
    let meta = std::fs::metadata(p).map_err(|e| format!("{p}: {e}"))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{p}: {} bytes exceeds the {MAX_FILE_BYTES} byte cap",
            meta.len()
        ));
    }
    std::fs::read(p)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| format!("{p}: {e}"))
}

fn trim(a: &Value) -> Result<String, String> {
    let query = a["query"].as_str().ok_or("need query")?;
    let text = match (a["text"].as_str(), a["path"].as_str()) {
        (Some(t), _) => t.to_string(),
        (None, Some(p)) => read_file(p)?,
        _ => return Err("need text or path".into()),
    };
    let budget = num(a, "budget_chars", 6000.0) as usize;
    let min = num(a, "min_relevance", 0.3);
    if text.len() <= budget {
        return Ok(text);
    }
    let size = (text.len() / 24).max(600);
    let chunks = chunk_text("text", &text, size);
    let n = chunks.len();
    let mid: Vec<&Chunk> = chunks.iter().skip(1).take(n.saturating_sub(2)).collect();

    let head_tail = |note: &str| {
        let h: String = text.chars().take(ANCHOR * 2).collect();
        let t: String = text
            .chars()
            .rev()
            .take(ANCHOR * 2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{h}\n[... middle trimmed ({note}) ...]\n{t}")
    };
    let scores = match score(query, &mid) {
        Ok(s) => s,
        Err(e) => return Ok(head_tail(&format!("head+tail fallback: {e}"))),
    };
    if scores.iter().all(Option::is_none) {
        return Ok(head_tail("head+tail fallback: no Kev scores"));
    }

    let mut order: Vec<usize> = (0..mid.len())
        .filter(|&i| scores[i].is_some_and(|s| s >= min))
        .collect();
    order.sort_by(|&x, &y| {
        scores[y]
            .partial_cmp(&scores[x])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut keep = std::collections::BTreeSet::new();
    let mut used = ANCHOR * 2 + 400;
    for i in order {
        if used + mid[i].text.len() <= budget {
            used += mid[i].text.len();
            keep.insert(i);
        }
    }

    let first = &chunks[0].text;
    let last = &chunks[n - 1].text;
    let mut out = clip(first, ANCHOR);
    let mut gap = first.chars().count().saturating_sub(ANCHOR);
    let flush = |out: &mut String, gap: &mut usize| {
        if *gap > 0 {
            out.push_str(&format!(
                "[... {} chars judged irrelevant by Kev, trimmed ...]\n",
                *gap
            ));
            *gap = 0;
        }
    };
    for (i, c) in mid.iter().enumerate() {
        if keep.contains(&i) {
            flush(&mut out, &mut gap);
            out.push_str(&c.text);
        } else {
            gap += c.text.chars().count();
        }
    }
    gap += last.chars().count().saturating_sub(ANCHOR);
    flush(&mut out, &mut gap);
    let tail: String = last
        .chars()
        .rev()
        .take(ANCHOR)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    out.push_str(&tail);
    Ok(out)
}

fn digest(a: &Value) -> Result<String, String> {
    let question = a["question"].as_str().ok_or("need question")?;
    let paths = a["paths"].as_array().ok_or("need paths")?;
    let budget = num(a, "budget_chars", 6000.0) as usize;
    let min = num(a, "min_relevance", 0.3);

    let mut all: Vec<Chunk> = Vec::new();
    let mut notes = Vec::new();
    for p in paths.iter().filter_map(Value::as_str) {
        match read_file(p) {
            Ok(t) => all.extend(chunk_text(p, &t, 900)),
            Err(e) => notes.push(e),
        }
    }
    if all.is_empty() {
        return Err(format!("nothing readable. {}", notes.join("; ")));
    }
    let total = all.len();

    // Lexical prefilter keeps Kev calls bounded on big inputs.
    let mut idx: Vec<usize> = (0..total).collect();
    if total > MAX_KEV_CALLS {
        idx.sort_by_key(|&i| std::cmp::Reverse(lexical(question, &all[i].text)));
        idx.truncate(MAX_KEV_CALLS);
    }
    let cands: Vec<&Chunk> = idx.iter().map(|&i| &all[i]).collect();
    let (scores, by) = match score(question, &cands) {
        Ok(s) if s.iter().any(Option::is_some) => (s, "kev"),
        _ => (
            cands
                .iter()
                .map(|c| Some(lexical(question, &c.text) as f64 / 10.0))
                .collect(),
            "lexical (Kev unavailable)",
        ),
    };
    let min = if by == "kev" { min } else { 0.1 };

    let mut order: Vec<usize> = (0..cands.len())
        .filter(|&i| scores[i].is_some_and(|s| s >= min))
        .collect();
    order.sort_by(|&x, &y| {
        scores[y]
            .partial_cmp(&scores[x])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut picked = Vec::new();
    let mut used = 0;
    for i in order {
        if used + cands[i].text.len() <= budget {
            used += cands[i].text.len();
            picked.push(i);
        }
    }
    picked.sort_by(|&x, &y| (&cands[x].src, cands[x].line).cmp(&(&cands[y].src, cands[y].line)));

    let mut out = format!(
        "digest: {} excerpts of {} chunks scanned, ranked by {by}\n",
        picked.len(),
        total
    );
    for n in notes {
        out.push_str(&format!("note: {n}\n"));
    }
    if picked.is_empty() {
        out.push_str("no chunk passed the relevance threshold; try a more specific question or lower min_relevance.\n");
    }
    for i in picked {
        let c = cands[i];
        out.push_str(&format!(
            "\n### {}:{} (relevance {:.2})\n{}",
            c.src,
            c.line,
            scores[i].unwrap_or(0.0),
            c.text
        ));
    }
    Ok(out)
}

fn pick_tier(a: &Value) -> Result<String, String> {
    let task = a["task"].as_str().ok_or("need task")?;
    let q = json!({
        "type": "choice",
        "instructions": "Which model tier does `task` need?",
        "criteria": {
            "small": "mechanical text work: summarize, extract, classify, reformat, search a log, draft a short message",
            "large": "needs multi-step reasoning, judgment, design, debugging or careful analysis"
        }
    });
    match kev_post(json!({"task": clip(task, 1000)}), "tier", q) {
        Ok(v) if matches!(v["choice"].as_str(), Some("small" | "large")) => {
            Ok(json!({"tier": v["choice"], "by": "kev"}).to_string())
        }
        _ => {
            let t = if task.len() < 400 { "small" } else { "large" };
            Ok(json!({"tier": t, "by": "size fallback"}).to_string())
        }
    }
}

fn handoff_latest() -> Result<String, String> {
    let rd = std::fs::read_dir(HANDOFF_DIR).map_err(|e| format!("{HANDOFF_DIR}: {e}"))?;
    let mut files: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    files.sort(); // UTC timestamp names sort chronologically
    let p = files.last().ok_or("no handoff files yet")?;
    let body = read_file(&p.to_string_lossy())?;
    Ok(format!("# {}\n\n{}", p.display(), clip(&body, 30_000)))
}

fn handoff_write(a: &Value) -> Result<String, String> {
    let text = a["text"].as_str().ok_or("need text")?;
    std::fs::create_dir_all(HANDOFF_DIR).map_err(|e| e.to_string())?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let (d, s) = (secs / 86_400, secs % 86_400);
    // civil-from-days (Howard Hinnant), UTC
    let z = d as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let name = format!(
        "{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}Z.md",
        s / 3600,
        s % 3600 / 60,
        s % 60
    );
    let path = format!("{HANDOFF_DIR}/{name}");
    std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))?;
    Ok(format!("wrote {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_keep_line_numbers() {
        let t = (1..=200)
            .map(|i| format!("line {i} {}", "x".repeat(40)))
            .collect::<Vec<_>>()
            .join("\n");
        let c = chunk_text("f", &t, 900);
        assert!(c.len() > 3);
        assert_eq!(c[0].line, 1);
        assert!(c[1].text.starts_with(&format!("line {} ", c[1].line)));
    }

    #[test]
    fn lexical_counts_distinct_words() {
        assert_eq!(
            lexical("find the race condition", "a RACE here, condition too"),
            2
        );
    }

    #[test]
    fn tools_listed_and_unknown_errors() {
        let listed = tools();
        assert_eq!(listed.as_array().unwrap().len(), 8);
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "search_queries")
        );
        assert!(call("nope", &json!({})).1);
    }

    #[test]
    fn trim_returns_short_text_untouched() {
        let (t, e) = call("trim", &json!({"query": "q", "text": "short"}));
        assert!(!e);
        assert_eq!(t, "short");
    }

    #[test]
    fn handoff_name_is_utc_and_sortable() {
        // 2026-10-10T18:20:59Z = 1791656459
        let secs = 1_791_656_459u64;
        let (d, s) = (secs / 86_400, secs % 86_400);
        let z = d as i64 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        assert_eq!((year, month, day, s / 3600), (2026, 10, 10, 18));
    }
}

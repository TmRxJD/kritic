//! Context management for agents that let a plugin own their context (Hermes' context engines).
//!
//! The same rule as the rest of kritic: nothing is generated. When a conversation has to shrink,
//! whole messages from its middle are **archived**: their content moves to a per-session archive
//! on disk and is replaced by a short stub naming a stable id, the size and an opening excerpt.
//! The agent can read any archived message back with the `kritic_recall` tool, so compaction is
//! lossless: nothing is summarised away, and nothing the model sees was written by another model.
//!
//! What goes first:
//! 1. duplicates: a tool result identical to a later one;
//! 2. oversized tool results (terminal dumps, file reads) larger than `big_chars`;
//! 3. if still over the target: the least relevant middle messages, ranked by Kev's
//!    `context_relevance` question against the current task (the latest user message plus any
//!    `/compress` focus), largest first on ties; without Kev, the oldest first.
//!
//! Structure is never changed: messages keep their role, order, `tool_call_id`, `tool_calls`
//! (tool-call arguments are execution records and stay byte-exact) and host fields such as
//! `message_uid`, so the result is always a valid OpenAI-format sequence. Stub ids are content
//! hashes, so re-archiving the same content yields the same stub and prompt caches stay stable.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::kev::Decider;

/// Rough tokens for a message: ~4 characters per token over its content and tool-call arguments.
pub fn est_tokens(m: &Value) -> usize {
    let content = text_of(m).len();
    let calls = m.get("tool_calls").map(|c| c.to_string().len()).unwrap_or(0);
    (content + calls) / 4 + 4
}

pub fn total_tokens(messages: &[Value]) -> usize {
    messages.iter().map(est_tokens).sum()
}

/// A message's text: a string content, or the text parts of a multi-part content.
pub fn text_of(m: &Value) -> String {
    match m.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn is_stub(m: &Value) -> bool {
    text_of(m).starts_with(STUB_PREFIX)
}

const STUB_PREFIX: &str = "[kritic archived ";

pub fn archive_id(role: &str, content: &str) -> String {
    let mut h = Sha256::new();
    h.update(role.as_bytes());
    h.update([0]);
    h.update(content.as_bytes());
    let d = h.finalize();
    format!("kr-{}", d.iter().take(6).map(|b| format!("{b:02x}")).collect::<String>())
}

fn stub(id: &str, role: &str, content: &str, why: &str) -> String {
    let preview: String = content.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
    let more = if content.chars().count() > 160 { "…" } else { "" };
    format!(
        "{STUB_PREFIX}this {role} message ({} tokens, {why}) as {id}. Opening: \"{preview}{more}\". Call kritic_recall with id \"{id}\" to read it in full.]",
        content.len() / 4
    )
}

// ------------------------------------------------------------------ archive

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Archived {
    pub id: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub content: String,
    pub tokens: usize,
    pub why: String,
    pub at: String,
}

/// One session's archive: `<home>/context/<session>/archive.jsonl`, append-only.
pub struct Archive {
    pub path: PathBuf,
}

impl Archive {
    pub fn for_session(home: &Path, session: &str) -> Self {
        let safe: String = session.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(96).collect();
        let safe = if safe.is_empty() { "default".to_string() } else { safe };
        Archive { path: home.join("context").join(safe).join("archive.jsonl") }
    }

    pub fn load(&self) -> Vec<Archived> {
        std::fs::read_to_string(&self.path).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
    }

    fn append(&self, items: &[Archived]) -> Result<(), String> {
        if items.is_empty() {
            return Ok(());
        }
        // Ids are content hashes: one entry per id, across calls and within this batch.
        let mut known: HashSet<String> = self.load().into_iter().map(|a| a.id).collect();
        let fresh: Vec<&Archived> = items.iter().filter(|a| known.insert(a.id.clone())).collect();
        if fresh.is_empty() {
            return Ok(());
        }
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.path).map_err(|e| format!("cannot open {}: {e}", self.path.display()))?;
        for a in fresh {
            writeln!(f, "{}", serde_json::to_string(a).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ compress

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompressInput {
    pub messages: Vec<Value>,
    /// Aim for at most this many tokens (estimated).
    pub target_tokens: usize,
    #[serde(default)]
    pub session_id: String,
    /// `/compress <focus>`: what the kept context must serve, in addition to the latest request.
    #[serde(default)]
    pub focus: Option<String>,
    /// Non-system head messages kept verbatim.
    #[serde(default = "three")]
    pub protect_first_n: usize,
    /// Tail messages kept verbatim.
    #[serde(default = "six")]
    pub protect_last_n: usize,
    /// Tool results larger than this are archived even under the target.
    #[serde(default = "big")]
    pub big_chars: usize,
    /// `prune`: only duplicates and oversized tool results, no model. `compress`: everything.
    #[serde(default = "compress_mode")]
    pub mode: String,
    /// Rank with Kev (when available). Off = oldest first.
    #[serde(default = "yes")]
    pub use_kev: bool,
    /// Most messages Kev ranks per pass, and the time allowed for it.
    #[serde(default = "rank_max")]
    pub rank_max: usize,
    #[serde(default = "rank_deadline")]
    pub rank_deadline_s: u64,
}

fn three() -> usize {
    3
}
fn six() -> usize {
    6
}
fn big() -> usize {
    8000
}
fn compress_mode() -> String {
    "compress".into()
}
fn yes() -> bool {
    true
}
fn rank_max() -> usize {
    24
}
fn rank_deadline() -> u64 {
    45
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CompressOutput {
    pub messages: Vec<Value>,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub archived: Vec<ArchivedRef>,
    /// How the least relevant messages were chosen: "kev", "recency" or "none needed".
    pub ranking: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArchivedRef {
    pub id: String,
    pub index: usize,
    pub tokens: usize,
    pub why: String,
}

/// The latest user request: what kept context has to serve.
fn current_task(messages: &[Value], focus: Option<&str>) -> String {
    let last_user = messages.iter().rev().find(|m| m["role"] == "user" && !is_stub(m)).map(text_of).unwrap_or_default();
    let mut q: String = last_user.chars().take(700).collect();
    if let Some(f) = focus.filter(|f| !f.trim().is_empty()) {
        q = format!("{q}\nFocus: {}", f.chars().take(280).collect::<String>());
    }
    q
}

pub async fn compress<D: Decider>(decider: Option<&D>, archive: &Archive, input: &CompressInput) -> CompressOutput {
    let mut messages = input.messages.clone();
    let before = total_tokens(&messages);
    let mut out = CompressOutput { tokens_before: before, ranking: "none needed".into(), ..Default::default() };
    let n = messages.len();
    // Protected: system messages, the first N others, and the last N messages.
    let mut protected = vec![false; n];
    let mut head_left = input.protect_first_n;
    for (i, m) in messages.iter().enumerate() {
        if m["role"] == "system" {
            protected[i] = true;
        } else if head_left > 0 {
            protected[i] = true;
            head_left -= 1;
        }
    }
    for p in protected.iter_mut().skip(n.saturating_sub(input.protect_last_n)) {
        *p = true;
    }
    // Multimodal messages (images, audio parts) are left alone: a text stub can't stand in for them.
    let text_only = |m: &Value| match m.get("content") {
        Some(Value::Array(parts)) => parts.iter().all(|p| p.get("type").and_then(Value::as_str).is_none_or(|t| t == "text")),
        _ => true,
    };
    let candidates: Vec<usize> =
        (0..n).filter(|&i| !protected[i] && !is_stub(&messages[i]) && !text_of(&messages[i]).is_empty() && text_only(&messages[i])).collect();
    let now = chrono::Local::now().to_rfc3339();
    let mut archived: Vec<Archived> = Vec::new();
    let archive_one = |messages: &mut Vec<Value>, i: usize, why: &str, archived: &mut Vec<Archived>, refs: &mut Vec<ArchivedRef>| {
        let m = &messages[i];
        let role = m["role"].as_str().unwrap_or("message").to_string();
        let content = text_of(m);
        let id = archive_id(&role, &content);
        archived.push(Archived {
            id: id.clone(),
            role: role.clone(),
            message_uid: m.get("message_uid").and_then(Value::as_str).map(String::from),
            tool_call_id: m.get("tool_call_id").and_then(Value::as_str).map(String::from),
            tokens: content.len() / 4,
            content: content.clone(),
            why: why.to_string(),
            at: now.clone(),
        });
        refs.push(ArchivedRef { id: id.clone(), index: i, tokens: content.len() / 4, why: why.to_string() });
        messages[i]["content"] = Value::String(stub(&id, &role, &content, why));
    };

    // 1. Duplicate tool results: keep the latest copy.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for &i in candidates.iter().rev() {
        if messages[i]["role"] != "tool" {
            continue;
        }
        let key = archive_id("tool", &text_of(&messages[i]));
        if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(key) {
            e.insert(i);
        } else {
            archive_one(&mut messages, i, "duplicate of a later tool result", &mut archived, &mut out.archived);
        }
    }
    // 2. Oversized tool results.
    for &i in &candidates {
        if messages[i]["role"] == "tool" && !is_stub(&messages[i]) && text_of(&messages[i]).len() > input.big_chars {
            archive_one(&mut messages, i, "large tool result", &mut archived, &mut out.archived);
        }
    }
    // 3. Least relevant middle messages until under the target.
    if input.mode == "compress" && total_tokens(&messages) > input.target_tokens {
        let open: Vec<usize> = candidates.iter().copied().filter(|&i| !is_stub(&messages[i])).collect();
        let (order, ranking, notes) = rank(decider, &messages, &open, input).await;
        out.ranking = ranking;
        out.notes.extend(notes);
        for i in order {
            if total_tokens(&messages) <= input.target_tokens {
                break;
            }
            archive_one(&mut messages, i, "least relevant to the current task", &mut archived, &mut out.archived);
        }
        if total_tokens(&messages) > input.target_tokens {
            out.notes.push(format!(
                "still ~{} tokens over the target: the protected head and tail ({} + {} messages) and system prompt are kept verbatim",
                total_tokens(&messages) - input.target_tokens,
                input.protect_first_n,
                input.protect_last_n
            ));
        }
    }
    if let Err(e) = archive.append(&archived) {
        // Without the archive a stub would point at nothing: keep the originals instead.
        out.notes.push(format!("archive not written ({e}); nothing was archived"));
        out.messages = input.messages.clone();
        out.archived.clear();
        out.tokens_after = before;
        return out;
    }
    out.archived.sort_by_key(|a| a.index);
    out.tokens_after = total_tokens(&messages);
    out.messages = messages;
    out
}

/// Archive order: least relevant first. Kev ranks the largest `rank_max` candidates (a message's
/// opening against the current task); the rest, and everything when Kev is unavailable, go
/// oldest first. Ties go to the larger message: it frees more.
async fn rank<D: Decider>(decider: Option<&D>, messages: &[Value], open: &[usize], input: &CompressInput) -> (Vec<usize>, String, Vec<String>) {
    let mut notes = Vec::new();
    let oldest_first = || open.to_vec();
    let Some(d) = decider.filter(|_| input.use_kev) else { return (oldest_first(), "recency".into(), notes) };
    let task = current_task(messages, input.focus.as_deref());
    if task.trim().is_empty() {
        notes.push("no user request to rank against; oldest first".into());
        return (oldest_first(), "recency".into(), notes);
    }
    let spec = crate::spec::get("context_relevance");
    let q = spec.wire();
    let mut by_size: Vec<usize> = open.to_vec();
    by_size.sort_by_key(|&i| std::cmp::Reverse(est_tokens(&messages[i])));
    let deadline = Instant::now() + Duration::from_secs(input.rank_deadline_s);
    let mut score: BTreeMap<usize, f64> = BTreeMap::new();
    for &i in by_size.iter().take(input.rank_max) {
        if Instant::now() >= deadline {
            notes.push(format!("ranking stopped at the {}s deadline after {} messages", input.rank_deadline_s, score.len()));
            break;
        }
        let chunk: String = text_of(&messages[i]).chars().take(1400).collect();
        let Ok(state) = spec.state(&[("query", task.chars().take(1000).collect()), ("chunk", chunk)]) else { continue };
        match tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), d.decide(&state, &q)).await {
            Ok(Ok(dec)) => {
                if let Some(a) = dec.answers.get("relevant") {
                    score.insert(i, a.value);
                }
            }
            Ok(Err(e)) => {
                notes.push(format!("Kev unavailable ({e}); oldest first"));
                return (oldest_first(), "recency".into(), notes);
            }
            Err(_) => {
                notes.push(format!("ranking stopped at the {}s deadline", input.rank_deadline_s));
                break;
            }
        }
    }
    if score.is_empty() {
        return (oldest_first(), "recency".into(), notes);
    }
    // Scored messages by relevance (lowest first), then the unscored ones oldest first.
    let mut scored: Vec<usize> = score.keys().copied().collect();
    scored.sort_by(|a, b| score[a].total_cmp(&score[b]).then(est_tokens(&messages[*b]).cmp(&est_tokens(&messages[*a]))));
    let rest = open.iter().copied().filter(|i| !score.contains_key(i));
    (scored.into_iter().chain(rest).collect(), "kev".into(), notes)
}

// ------------------------------------------------------------------ recall

#[derive(Debug, Clone, Serialize, Default)]
pub struct RecallOutput {
    pub found: Vec<Archived>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Characters of content cut to fit `max_chars`.
    pub truncated_chars: usize,
}

/// Archived messages by id, or every archived message containing `query` (case-insensitive),
/// newest first, within `max_chars` of content.
pub fn recall(archive: &Archive, id: Option<&str>, query: Option<&str>, max_chars: usize) -> RecallOutput {
    let all = archive.load();
    let mut out = RecallOutput::default();
    let mut found: Vec<Archived> = match (id, query) {
        (Some(id), _) => all.into_iter().filter(|a| a.id == id).collect(),
        (None, Some(q)) if !q.trim().is_empty() => {
            let q = q.to_lowercase();
            all.into_iter().rev().filter(|a| a.content.to_lowercase().contains(&q)).collect()
        }
        _ => {
            out.error = Some("give an id (from an archive stub) or a query".into());
            return out;
        }
    };
    if found.is_empty() {
        out.error = Some(match id {
            Some(id) => format!("no archived message {id} in this session"),
            None => "no archived message matches".into(),
        });
        return out;
    }
    let mut left = max_chars;
    for a in &mut found {
        if a.content.len() > left {
            let keep = a.content.char_indices().take_while(|(i, _)| *i < left).map(|(i, c)| i + c.len_utf8()).last().unwrap_or(0);
            out.truncated_chars += a.content.len() - keep;
            a.content.truncate(keep);
        }
        left = left.saturating_sub(a.content.len());
    }
    found.retain(|a| !a.content.is_empty());
    out.found = found;
    out
}

/// The tool schema the Hermes plugin exposes for `recall`.
pub fn recall_tool_schema() -> Value {
    json!({
        "name": "kritic_recall",
        "description": "Read back conversation content that kritic archived to keep the context small. \
                        Archive stubs look like `[kritic archived this tool message (…) as kr-…]`. \
                        Pass the stub's id for that message, or a query to search every archived message.",
        "parameters": {
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "An archive id such as kr-1a2b3c4d5e6f"},
                "query": {"type": "string", "description": "Case-insensitive text to search archived content for"},
                "max_chars": {"type": "integer", "description": "Most characters to return (default 20000)"}
            }
        }
    })
}

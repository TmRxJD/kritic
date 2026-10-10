//! Candidate identification: Kev decides which files a task needs, so the frontier model reads
//! three files instead of searching a repository.
//!
//! The pipeline is deliberately cheap and mostly free: a lexical prefilter over the whole repo
//! (paths, defined names, opening lines) narrows thousands of files to a few dozen, Kev answers
//! one typed yes/no per survivor, and the score is a fixed blend of both. When Kev is offline the
//! lexical ranking is returned on its own and says so, so the tool never returns nothing.
//! The task text also gets a typed checklist of requirements that are easy to forget.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;

use crate::kev::Decider;

const MAX_FILES: usize = 6000;
const MAX_FILE_BYTES: u64 = 400_000;
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    ".venv",
    "venv",
    "__pycache__",
    "vendor",
    "coverage",
    ".next",
    "Library",
    "Temp",
    "obj",
    "bin",
];
const SKIP_FILES: &[&str] = &[
    "package-lock.json",
    "Cargo.lock",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "uv.lock",
    "composer.lock",
];
const CODE_EXT: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "cs", "c", "h", "cpp",
    "hpp", "swift", "rb", "php", "lua", "gd", "svelte", "vue", "html", "css", "scss", "json",
    "toml", "yaml", "yml", "md", "sql", "sh", "ps1", "gleam", "ex", "exs", "uxml", "uss", "shader",
];

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Candidate {
    pub path: String,
    /// Blend of Kev's probability and the lexical score, 0-1.
    pub score: f64,
    /// Keyword-match score alone, 0-1 (what a text search would rank).
    pub keyword: f64,
    /// Kev's probability that the file is needed; absent when Kev did not judge it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kev: Option<f64>,
    /// What the file defines (the names that matched the task come first).
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Requirement {
    pub id: String,
    pub probability: f64,
    /// What to do about it.
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Default, schemars::JsonSchema)]
pub struct LocateResult {
    pub tool: String,
    pub advisory: bool,
    pub kev_ok: bool,
    pub files_in_repo: usize,
    pub files_considered: usize,
    /// The files to read first, best first.
    pub candidates: Vec<Candidate>,
    /// True when the top candidate is clearly ahead; false means search further yourself.
    pub confident: bool,
    /// Requirements the task implies that are easy to forget (only those Kev found likely).
    pub checklist: Vec<Requirement>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ------------------------------------------------------------------ repository scan

/// Files the repository tracks: `git ls-files`, else a bounded walk.
pub fn list_files(base: &Path) -> Vec<PathBuf> {
    let git = std::process::Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(base)
        .stdin(std::process::Stdio::null())
        .output();
    let mut out: Vec<PathBuf> = match git {
        Ok(o) if o.status.success() => o
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()))
            .collect(),
        _ => {
            let mut v = Vec::new();
            walk(base, base, &mut v, 0);
            v
        }
    };
    out.retain(|p| wanted(p));
    out.truncate(MAX_FILES);
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 12 || out.len() >= MAX_FILES {
        return;
    }
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                walk(root, &p, out, depth + 1);
            }
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
}

fn wanted(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if SKIP_FILES.contains(&name) || name.ends_with(".min.js") || name.ends_with(".map") {
        return false;
    }
    if p.components()
        .any(|c| SKIP_DIRS.contains(&c.as_os_str().to_str().unwrap_or("")))
    {
        return false;
    }
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| CODE_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

struct Doc {
    path: String,
    /// Tokens weighted by where they occur.
    terms: HashMap<String, f64>,
    names: Vec<String>,
    head: String,
    len: f64,
}

/// `parseHTTPRequest` → parse, http, request; `snake_case` → snake, case. Lowercase, 2+ chars.
pub fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_alphanumeric() {
            let boundary = i > 0
                && c.is_uppercase()
                && (chars[i - 1].is_lowercase()
                    || chars.get(i + 1).is_some_and(|n| n.is_lowercase())
                        && chars[i - 1].is_uppercase());
            if boundary && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|t| t.len() >= 2 && !STOP.contains(&t.as_str()));
    out
}

const STOP: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "add", "make", "new", "use", "fn",
    "let", "var", "const", "pub", "self", "of", "to", "in", "is", "it", "on", "or", "an", "as",
    "at", "be", "by", "if", "do", "we", "so", "no", "not", "are", "can", "when", "then", "than",
    "should", "must", "will", "also", "each", "all", "any", "its",
];

fn defs() -> &'static Regex {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?m)^\s*(?:export\s+|pub(?:\([a-z]+\))?\s+|public\s+|private\s+|protected\s+|internal\s+|static\s+|async\s+|abstract\s+|final\s+|sealed\s+|partial\s+)*(?:fn|function|def|class|struct|enum|trait|interface|type|impl|mod|const|func|record|module|namespace|object|sub|proc)\s+([A-Za-z_][A-Za-z0-9_]*)",
        )
        .unwrap()
    })
}

fn doc_for(base: &Path, rel: &Path) -> Option<Doc> {
    let full = base.join(rel);
    let meta = std::fs::metadata(&full).ok()?;
    if meta.len() > MAX_FILE_BYTES || meta.len() == 0 {
        return None;
    }
    let text = std::fs::read_to_string(&full).ok()?;
    let path = rel.to_string_lossy().replace('\\', "/");
    let mut terms: HashMap<String, f64> = HashMap::new();
    let mut add = |toks: Vec<String>, w: f64| {
        for t in toks {
            *terms.entry(t).or_insert(0.0) += w;
        }
    };
    add(tokens(&path), 3.0);
    let names: Vec<String> = defs()
        .captures_iter(&text)
        .map(|c| c[1].to_string())
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    let names: Vec<String> = names
        .into_iter()
        .filter(|n| seen.insert(n.clone()))
        .take(40)
        .collect();
    for n in &names {
        add(tokens(n), 2.0);
    }
    let head: String = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(6)
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(360)
        .collect();
    add(tokens(&head), 1.5);
    // Body words count once each, and lightly: they separate files that mention a topic in passing.
    let body: HashSet<String> = tokens(&text.chars().take(60_000).collect::<String>())
        .into_iter()
        .collect();
    for t in body {
        *terms.entry(t).or_insert(0.0) += 0.3;
    }
    let len = terms.values().sum::<f64>().max(1.0);
    Some(Doc {
        path,
        terms,
        names,
        head,
        len,
    })
}

/// BM25-flavoured lexical scores for every document against the task's tokens.
fn lexical(docs: &[Doc], query: &[String]) -> Vec<f64> {
    let n = docs.len() as f64;
    let avg = docs.iter().map(|d| d.len).sum::<f64>() / n.max(1.0);
    let q: HashSet<&String> = query.iter().collect();
    let mut df: HashMap<&str, f64> = HashMap::new();
    for d in docs {
        for t in &q {
            if d.terms.contains_key(t.as_str()) {
                *df.entry(t.as_str()).or_insert(0.0) += 1.0;
            }
        }
    }
    docs.iter()
        .map(|d| {
            q.iter()
                .map(|t| {
                    let tf = d.terms.get(t.as_str()).copied().unwrap_or(0.0);
                    if tf == 0.0 {
                        return 0.0;
                    }
                    let idf = ((n - df[t.as_str()] + 0.5) / (df[t.as_str()] + 0.5) + 1.0).ln();
                    idf * tf * 2.2 / (tf + 1.2 * (0.25 + 0.75 * d.len / avg))
                })
                .sum()
        })
        .collect()
}

fn card(d: &Doc, query: &HashSet<String>) -> String {
    let mut names: Vec<&String> = d.names.iter().collect();
    // names that overlap the task first
    names.sort_by_key(|n| {
        std::cmp::Reverse(tokens(n).iter().filter(|t| query.contains(*t)).count())
    });
    let names = names
        .into_iter()
        .take(14)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}\ndefines: {}\n{}", d.path, names, d.head)
        .chars()
        .take(1150)
        .collect()
}

// ------------------------------------------------------------------ locate

fn action(id: &str) -> &'static str {
    match id {
        "new_surface" => {
            "Add a visible entry point (menu item, button, route or command), register it, and check it can be reached from the start screen."
        }
        "data_format" => {
            "Update every reader and writer of the format, and add or adjust a migration or compatibility test."
        }
        "security" => {
            "Validate and bound the input, keep secrets out of the code, and run the security scan before finishing."
        }
        "behavior" => "Add or extend a test that fails without the change.",
        "unclear" => "Ask what done looks like before building.",
        _ => "",
    }
}

pub struct LocateOptions {
    pub top_k: usize,
    /// Files Kev judges after the lexical prefilter.
    pub prefilter: usize,
    pub deadline: Duration,
    pub checklist: bool,
}

pub async fn locate<D: Decider>(d: &D, base: &Path, task: &str, o: &LocateOptions) -> LocateResult {
    let mut r = LocateResult {
        tool: "locate".into(),
        advisory: true,
        ..Default::default()
    };
    if task.trim().is_empty() {
        r.error = Some("empty task".into());
        return r;
    }
    if !base.is_dir() {
        r.error = Some(format!("{} is not a directory", base.display()));
        return r;
    }
    let files = list_files(base);
    r.files_in_repo = files.len();
    let docs: Vec<Doc> = files.iter().filter_map(|f| doc_for(base, f)).collect();
    if docs.is_empty() {
        r.error = Some("no source files found".into());
        return r;
    }
    let query_tokens = tokens(task);
    let scores = lexical(&docs, &query_tokens);
    let max = scores.iter().copied().fold(0.0, f64::max);
    let mut order: Vec<usize> = (0..docs.len()).collect();
    order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    let pool: Vec<usize> = order
        .iter()
        .copied()
        .take(o.prefilter.max(o.top_k))
        .filter(|i| scores[*i] > 0.0 || max == 0.0)
        .collect();
    r.files_considered = pool.len();

    // Kev judges each survivor; a deadline keeps a slow model from stalling the caller.
    let spec = crate::spec::get("file_relevance");
    let wire = spec.wire();
    let qset: HashSet<String> = query_tokens.iter().cloned().collect();
    let mut kev: HashMap<usize, f64> = HashMap::new();
    let start = Instant::now();
    let mut kev_error = None;
    for &i in &pool {
        if start.elapsed() > o.deadline {
            kev_error = Some(format!(
                "Kev deadline ({}s) reached after {} of {} files",
                o.deadline.as_secs(),
                kev.len(),
                pool.len()
            ));
            break;
        }
        let task_clip: String = task.chars().take(800).collect();
        let state = match spec.state(&[("task", task_clip), ("file", card(&docs[i], &qset))]) {
            Ok(s) => s,
            Err(e) => {
                kev_error = Some(e);
                break;
            }
        };
        match d.decide(&state, &wire).await {
            Ok(dec) => {
                kev.insert(i, dec.answers["needed"].value);
            }
            Err(e) => {
                kev_error = Some(e);
                break;
            }
        }
    }
    r.kev_ok = !kev.is_empty() && kev.len() == pool.len();

    // Kev leads (0.7); the lexical score keeps a tie or a Kev miss honest (0.3).
    let blended = |i: usize| {
        let lex = if max > 0.0 { scores[i] / max } else { 0.0 };
        match kev.get(&i) {
            Some(k) => 0.7 * k + 0.3 * lex,
            None => 0.3 * lex,
        }
    };
    let mut ranked: Vec<usize> = pool.clone();
    ranked.sort_by(|a, b| blended(*b).total_cmp(&blended(*a)));
    r.candidates = ranked
        .iter()
        .take(o.top_k)
        .map(|&i| {
            let mut names: Vec<&String> = docs[i].names.iter().collect();
            names.sort_by_key(|n| {
                std::cmp::Reverse(tokens(n).iter().filter(|t| qset.contains(*t)).count())
            });
            Candidate {
                path: docs[i].path.clone(),
                score: (blended(i) * 1000.0).round() / 1000.0,
                keyword: if max > 0.0 {
                    (scores[i] / max * 1000.0).round() / 1000.0
                } else {
                    0.0
                },
                kev: kev.get(&i).map(|k| (k * 1000.0).round() / 1000.0),
                names: names.into_iter().take(6).cloned().collect(),
            }
        })
        .collect();
    r.confident = match (r.candidates.first(), r.candidates.get(1)) {
        (Some(a), Some(b)) => {
            a.kev.is_some() && a.score >= 0.5 && a.score - b.score >= 0.05 || a.score >= 0.75
        }
        (Some(a), None) => a.score >= 0.5,
        _ => false,
    };
    if r.kev_ok {
        r.note = if r.confident {
            String::new()
        } else {
            "No file stands out; read the top few, then search for what they miss.".into()
        };
    } else {
        r.note = format!(
            "Ranked by keyword match only ({}); treat as a search result.",
            kev_error.unwrap_or_else(|| "Kev unavailable".into())
        );
    }

    if o.checklist && !kev.is_empty() {
        r.checklist = requirements(d, task).await;
    }
    r
}

/// Typed checklist for the task text; empty when Kev cannot answer.
pub async fn requirements<D: Decider>(d: &D, task: &str) -> Vec<Requirement> {
    let spec = crate::spec::get("task_requirements");
    let Ok(state) = spec.state(&[("task", task.chars().take(1500).collect())]) else {
        return vec![];
    };
    let Ok(dec) = d.decide(&state, &spec.wire()).await else {
        return vec![];
    };
    let mut out: Vec<Requirement> = dec
        .answers
        .iter()
        .filter(|(id, a)| a.value >= 0.5 && !action(id).is_empty())
        .map(|(id, a)| Requirement {
            id: id.clone(),
            probability: (a.value * 100.0).round() / 100.0,
            action: action(id).into(),
        })
        .collect();
    out.sort_by(|a, b| b.probability.total_cmp(&a.probability));
    out
}

// ------------------------------------------------------------------ decide

#[derive(Debug, Clone, Serialize, Default, schemars::JsonSchema)]
pub struct DecideResult {
    pub tool: String,
    pub advisory: bool,
    pub kev_ok: bool,
    /// The option Kev picked (a yes/no question answers "yes" or "no").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    pub probability: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probabilities: Option<std::collections::BTreeMap<String, f64>>,
    /// True when Kev is not sure enough: decide it yourself.
    pub escalate: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One typed decision for the caller: pick one of `options` (id, meaning), or answer yes/no when
/// there are none. Below `threshold` the answer is returned with `escalate: true`.
pub async fn decide<D: Decider>(
    d: &D,
    question: &str,
    context: &str,
    options: &[(String, String)],
    threshold: f64,
) -> DecideResult {
    let mut r = DecideResult {
        tool: "decide".into(),
        advisory: true,
        ..Default::default()
    };
    if question.trim().is_empty() {
        r.error = Some("empty question".into());
        return r;
    }
    if options.len() == 1 || options.len() > 12 {
        r.error = Some("give 2 to 12 options, or none for a yes/no question".into());
        return r;
    }
    let ask = if options.is_empty() {
        serde_json::json!({"type": "noul",
            "instructions": "Considering `context`, is the answer to `question` yes?",
            "yes": "the answer to `question` is yes, judged from `context`",
            "no": "the answer to `question` is no, judged from `context`"})
    } else {
        serde_json::json!({"type": "choice",
            "instructions": "Considering `context`, which option answers `question`?",
            "options": options.iter().map(|(k, v)| serde_json::json!([k, v])).collect::<Vec<_>>()})
    };
    let text = serde_json::json!({
        "name": "decide",
        "purpose": "One decision the caller asked for. Advice only.",
        "fields": [
            {"name": "question", "kind": {"kind": "text", "max_chars": 600}, "meaning": "What has to be decided."},
            {"name": "context", "kind": {"kind": "text", "max_chars": 1400}, "meaning": "The facts the decision rests on."}
        ],
        "questions": [{"id": "d", "ask": ask, "gate": {"gate": "advisory", "why": "advice; the caller decides"}}]
    })
    .to_string();
    let spec = match crate::spec::load(&text) {
        Ok(s) => s,
        Err(e) => {
            r.error = Some(e.join("; "));
            return r;
        }
    };
    let state = match spec.state(&[
        ("question", question.to_string()),
        ("context", context.to_string()),
    ]) {
        Ok(s) => s,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    match d.decide(&state, &spec.wire()).await {
        Err(e) => r.error = Some(e),
        Ok(dec) => {
            let a = &dec.answers["d"];
            r.kev_ok = true;
            if options.is_empty() {
                r.choice = Some(if a.value >= 0.5 { "yes" } else { "no" }.into());
                r.probability = ((if a.value >= 0.5 {
                    a.value
                } else {
                    1.0 - a.value
                }) * 1000.0)
                    .round()
                    / 1000.0;
            } else {
                r.choice = a.choice.clone();
                r.probability = (a.value * 1000.0).round() / 1000.0;
                r.probabilities = a.probabilities.clone();
            }
            r.escalate = r.probability < threshold;
            if r.escalate {
                r.note = format!(
                    "Kev is only {:.0}% sure (threshold {:.0}%): decide it yourself.",
                    r.probability * 100.0,
                    threshold * 100.0
                );
            }
        }
    }
    r
}

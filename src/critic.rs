//! Kev-backed critic: chunk the artifact, ask typed questions per chunk, aggregate with fixed
//! rules. Kev supplies calibrated probabilities; approval is computed here from thresholds.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

use crate::chunk::{Chunk, chunk_text, digest};
use crate::config::Config;
use crate::kev::Decider;
use crate::spec::DecisionSpec;

pub const MAX_CHUNKS: usize = 60;
/// A severity label's position in the spec's scale (0 = "none"), so the definitions live in one
/// place: specs/code_chunk.json.
fn severity_rank(s: &str) -> usize {
    match unit_spec("files").question("severity").map(|q| &q.ask) {
        Some(crate::spec::Ask::Choice { options, .. }) => {
            options.iter().position(|(k, _)| k == s).unwrap_or(0)
        }
        _ => 0,
    }
}

/// The spec a unit is judged with: diff hunks and code excerpts are different states.
pub fn unit_spec(mode: &str) -> &'static DecisionSpec {
    crate::spec::get(if mode == "diff" {
        "code_change"
    } else {
        "code_chunk"
    })
}

/// Completeness is judged on code when the change fits, else on a digest; they are different
/// states with different calibration, so they are different specs.
pub fn completeness_spec(basis: &str) -> &'static DecisionSpec {
    crate::spec::get(if basis == "digest" {
        "completeness_digest"
    } else {
        "completeness"
    })
}

fn completeness_state(task: &str, basis: &str, text: String) -> Result<Value, String> {
    let field = if basis == "digest" {
        "change_digest"
    } else {
        "change_summary"
    };
    completeness_spec(basis).state(&[("task", task.chars().take(1500).collect()), (field, text)])
}

/// A diff hunk (or a slice of a long one) to be scored.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub file: String,
    pub new_start: usize,
    pub text: String,
}

/// Split unified diff text into per-file hunks, each at most `max_chars` (long hunks are sliced
/// on line boundaries, keeping the @@ header on every slice).
pub fn parse_diff(diff: &str, max_chars: usize) -> Vec<Hunk> {
    let mut out = Vec::new();
    let mut file = String::from("<diff>");
    let mut header = String::new();
    let mut start = 0usize;
    let mut body = String::new();
    let mut old_file = String::new();
    let (mut old_left, mut new_left, mut new_cursor) = (0usize, 0usize, 0usize);
    let filename = |raw: &str| {
        let name = raw.split('\t').next().unwrap_or(raw);
        name.strip_prefix("a/")
            .or_else(|| name.strip_prefix("b/"))
            .unwrap_or(name)
            .to_string()
    };
    let flush = |file: &str, header: &str, start: usize, body: &mut String, out: &mut Vec<Hunk>| {
        if body
            .lines()
            .any(|l| l.starts_with('+') || l.starts_with('-'))
        {
            out.push(Hunk {
                file: file.into(),
                new_start: start,
                text: format!("{header}\n{body}"),
            });
        }
        body.clear();
    };
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush(&file, &header, start, &mut body, &mut out);
            file = rest.rsplit(" b/").next().unwrap_or(rest).to_string();
            header.clear();
            old_left = 0;
            new_left = 0;
        } else if line.starts_with("@@") {
            flush(&file, &header, start, &mut body, &mut out);
            header = line.to_string();
            start = line
                .split_whitespace()
                .find_map(|t| t.strip_prefix('+'))
                .and_then(|t| t.split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            let count = |prefix: char| {
                line.split_whitespace()
                    .find_map(|part| part.strip_prefix(prefix))
                    .map(|part| {
                        part.split_once(',')
                            .map_or(1, |(_, n)| n.parse().unwrap_or(0))
                    })
                    .unwrap_or(0)
            };
            old_left = count('-');
            new_left = count('+');
            new_cursor = start;
        } else if old_left > 0 || new_left > 0 {
            // Consume valid hunk data before interpreting header-like source text.
            if body.len() + line.len() + header.len() + 2 > max_chars && !body.is_empty() {
                flush(&file, &header, start, &mut body, &mut out);
                start = new_cursor;
            }
            match line.as_bytes().first() {
                Some(b'+') => {
                    new_left = new_left.saturating_sub(1);
                    new_cursor = new_cursor.saturating_add(1);
                }
                Some(b'-') => {
                    old_left = old_left.saturating_sub(1);
                }
                Some(b' ') => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    new_cursor = new_cursor.saturating_add(1);
                }
                Some(b'\\') => {}
                _ => continue,
            }
            body.push_str(line);
            body.push('\n');
        } else if let Some(path) = line.strip_prefix("--- ") {
            flush(&file, &header, start, &mut body, &mut out);
            old_file = filename(path);
            header.clear();
        } else if let Some(path) = line.strip_prefix("+++ ") {
            flush(&file, &header, start, &mut body, &mut out);
            let next = filename(path);
            file = if next == "/dev/null" {
                old_file.clone()
            } else {
                next
            };
            header.clear();
        } else if line.starts_with("index ")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
        {
            continue;
        } else if !header.is_empty() && line.starts_with("\\ No newline") {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&file, &header, start, &mut body, &mut out);
    out
}

/// Added lines per file, for the completeness digest in diff mode.
pub fn added_by_file(hunks: &[Hunk]) -> Vec<(String, String)> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for h in hunks {
        let e = map.entry(h.file.clone()).or_default();
        for l in h.text.lines().filter(|l| l.starts_with('+')) {
            e.push_str(&l[1..]);
            e.push('\n');
        }
    }
    map.into_iter().collect()
}

/// Score only what changed. Kev separates buggy from clean hunks better when asked whether the
/// change introduces a bug than when judging whole excerpts of real code.
pub async fn critic_diff<D: Decider>(
    d: &D,
    cfg: &Config,
    task: &str,
    diff: &str,
    revision: u32,
) -> CriticResult {
    let mut r = CriticResult {
        tool: "critic_score".into(),
        mode: "diff".into(),
        advisory: true,
        model: cfg.model.clone(),
        revision,
        max_revisions: cfg.max_revisions,
        revise_allowed: revision < cfg.max_revisions,
        ..Default::default()
    };
    let task = if task.trim().is_empty() {
        "(no task given: judge the change on its own)"
    } else {
        task
    };
    let mut hunks = parse_diff(diff, cfg.chunk_chars);
    if hunks.is_empty() {
        r.error = Some("empty diff (no changed lines)".into());
        return r;
    }
    if hunks.len() > MAX_CHUNKS {
        r.chunks_unscored = hunks.len() - MAX_CHUNKS;
        hunks.truncate(MAX_CHUNKS);
    }
    let spec = unit_spec("diff");
    let qs = spec.wire();
    let mut verdicts = Vec::new();
    let mut weights = Vec::new();
    let mut queue: std::collections::VecDeque<(Hunk, u8)> =
        hunks.iter().cloned().map(|h| (h, 0)).collect();
    let total = queue.len();
    let deadline = Deadline::new(cfg);
    while let Some((h, depth)) = queue.pop_front() {
        if let Some(e) = deadline.passed(verdicts.len(), total) {
            r.error = Some(e);
            return r;
        }
        let h = &h;
        crate::events::emit(
            "progress",
            json!({"tool": "critic_score", "unit": format!("{}:+{}", h.file, h.new_start), "done": verdicts.len(), "total": total}),
        );
        let label = format!("{}:+{}", h.file, h.new_start);
        // The request is not part of this state: measured, its wording moved severity by +0.3 on
        // identical code. Fit to the request is the completeness spec's question.
        // Removed lines stay out: they leaked into judgments of the added ones.
        let added: String = h
            .text
            .lines()
            .filter(|l| !l.starts_with('-'))
            .collect::<Vec<_>>()
            .join("\n");
        let state = match spec.state(&[("file", h.file.clone()), ("added", added)]) {
            Ok(s) => s,
            Err(e) => {
                r.error = Some(format!("{e} (at {label})"));
                return r;
            }
        };
        match deadline
            .bound(d.decide(&state, &qs), verdicts.len(), total)
            .await
        {
            Err(e) => {
                r.error = Some(format!("{e} (at {label})"));
                return r;
            }
            Ok(dec) => {
                r.latency_ms += dec.latency_ms;
                r.kev_input_tokens += dec.input_tokens;
                // laya silently truncated this row: split the hunk and re-score (max 2 levels)
                if dec.truncated
                    && depth < 2
                    && let Some((a, b)) = halve_hunk(h)
                {
                    queue.push_front((b, depth + 1));
                    queue.push_front((a, depth + 1));
                    continue;
                }
                let a = &dec.answers;
                verdicts.push((
                    ChunkVerdict {
                        location: label,
                        p_correct: 1.0 - a["defect"].value,
                        severity: a["severity"].choice.clone().unwrap_or_default(),
                        p_severe: p_severe(&a["severity"]),
                        p_security_issue: a["security"].value,
                        p_unfinished: a["unfinished"].value,
                        p_reviewer_injection: a["reviewer_text"].value,
                        quality: round2(a["quality"].value / 4.0 * 10.0),
                        truncated: dec.truncated,
                    },
                    a["defect"].confidence,
                ));
                weights.push(h.text.len() as f64);
            }
        }
    }
    let added = added_by_file(&hunks);
    let total: usize = added.iter().map(|(_, t)| t.len()).sum();
    let (text, basis) = if total <= cfg.chunk_chars {
        (
            hunks
                .iter()
                .map(|h| h.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            "code",
        )
    } else {
        (digest(&added, cfg.chunk_chars), "digest")
    };
    let state = match completeness_state(task, basis, text) {
        Ok(s) => s,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    let p_complete = match deadline
        .bound(
            d.decide(&state, &completeness_spec(basis).wire()),
            verdicts.len(),
            hunks.len(),
        )
        .await
    {
        Err(e) => {
            r.error = Some(e);
            return r;
        }
        Ok(dec) => {
            r.latency_ms += dec.latency_ms;
            dec.answers["complete"].value
        }
    };
    aggregate(&mut r, &verdicts, &weights, p_complete, basis);
    r
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ChunkVerdict {
    /// file:start-end
    pub location: String,
    pub p_correct: f64,
    /// Most likely severity label (argmax over 4 options; can win at ~0.3).
    pub severity: String,
    /// P(major) + P(critical). The severity rule uses this, not the argmax label.
    pub p_severe: f64,
    pub p_security_issue: f64,
    pub p_unfinished: f64,
    pub p_reviewer_injection: f64,
    /// 0-10
    pub quality: f64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct CriticResult {
    pub tool: String,
    /// "files": whole files were scored; "diff": only changed hunks were scored.
    pub mode: String,
    pub advisory: bool,
    pub model: String,
    /// False when Kev could not be reached or returned invalid output. Then every score is null.
    pub kev_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 0-10, from min p(correct) across chunks.
    pub correctness: Option<f64>,
    /// 0-10, from p(complete) on the whole change (or its digest).
    pub completeness: Option<f64>,
    /// 0-10, size-weighted mean of the per-chunk quality score.
    pub quality: Option<f64>,
    pub p_correct_min: Option<f64>,
    pub p_complete: Option<f64>,
    pub p_security_issue_max: Option<f64>,
    pub p_unfinished_max: Option<f64>,
    pub p_reviewer_injection_max: Option<f64>,
    /// Argmax label of the worst chunk (advisory; severity does not gate approval).
    pub worst_severity: Option<String>,
    /// Max over units of P(major) + P(critical).
    pub p_severe_max: Option<f64>,
    /// Mean Kev confidence over the correctness answers.
    pub confidence: Option<f64>,
    /// Computed from thresholds; null when Kev is offline or the artifact was too large to score fully.
    pub approval: Option<bool>,
    /// Why approval is false/null, one line per failed rule. Deterministic, not model text.
    pub reasons: Vec<String>,
    /// Chunks that failed any rule, with their probabilities. Kev does not explain; read these spots.
    pub flagged_chunks: Vec<ChunkVerdict>,
    /// Major/critical labels are advisory, but retain their locations even when no gate fails.
    pub advisory_chunks: Vec<ChunkVerdict>,
    /// Explains scores that must not be interpreted as deterministic task verification.
    pub advisory_notes: Vec<String>,
    pub chunks_scored: usize,
    pub chunks_unscored: usize,
    pub truncated_chunks: usize,
    /// "digest" when completeness was judged from a file/definition summary instead of full code.
    pub completeness_basis: String,
    pub revision: u32,
    pub max_revisions: u32,
    pub revise_allowed: bool,
    pub latency_ms: u64,
    /// Kev tokens processed for this call.
    pub kev_input_tokens: u64,
    /// The calibrated spec questions that decided approval (spec.question). Everything else
    /// in this result is reported, not decisive.
    pub gates: Vec<String>,
}

/// Score a set of (label, text) files against `task`.
pub async fn critic<D: Decider>(
    d: &D,
    cfg: &Config,
    task: &str,
    files: &[(String, String)],
    revision: u32,
) -> CriticResult {
    let mut r = CriticResult {
        tool: "critic_score".into(),
        mode: "files".into(),
        advisory: true,
        model: cfg.model.clone(),
        revision,
        max_revisions: cfg.max_revisions,
        revise_allowed: revision < cfg.max_revisions,
        ..Default::default()
    };
    let task = if task.trim().is_empty() {
        "(no task given: judge the code on its own)"
    } else {
        task
    };
    let mut chunks: Vec<Chunk> = files
        .iter()
        .flat_map(|(f, t)| chunk_text(f, t, cfg.chunk_chars))
        .collect();
    if chunks.is_empty() {
        r.error = Some("empty artifact (nothing readable to score)".into());
        return r;
    }
    if chunks.len() > MAX_CHUNKS {
        r.chunks_unscored = chunks.len() - MAX_CHUNKS;
        chunks.truncate(MAX_CHUNKS);
    }

    let spec = unit_spec("files");
    let qs = spec.wire();
    let mut verdicts = Vec::new();
    let mut weights = Vec::new();
    let mut queue: std::collections::VecDeque<(Chunk, u8)> =
        chunks.iter().cloned().map(|c| (c, 0)).collect();
    let total = queue.len();
    let deadline = Deadline::new(cfg);
    while let Some((c, depth)) = queue.pop_front() {
        if let Some(e) = deadline.passed(verdicts.len(), total) {
            r.error = Some(e);
            return r;
        }
        let c = &c;
        crate::events::emit(
            "progress",
            json!({"tool": "critic_score", "unit": c.label(), "done": verdicts.len(), "total": total}),
        );
        let state = match spec.state(&[
            (
                "file",
                format!(
                    "{} (lines {}-{} of {})",
                    c.file, c.start_line, c.end_line, c.total_lines
                ),
            ),
            ("code", c.text.clone()),
        ]) {
            Ok(s) => s,
            Err(e) => {
                r.error = Some(format!("{e} (at {})", c.label()));
                return r;
            }
        };
        match deadline
            .bound(d.decide(&state, &qs), verdicts.len(), total)
            .await
        {
            Err(e) => {
                r.error = Some(format!("{e} (at {})", c.label()));
                return r;
            }
            Ok(dec) => {
                r.latency_ms += dec.latency_ms;
                r.kev_input_tokens += dec.input_tokens;
                // laya silently truncated this row: split and re-score (max 2 levels).
                if dec.truncated
                    && depth < 2
                    && let Some((a, b)) = halve(c)
                {
                    queue.push_front((b, depth + 1));
                    queue.push_front((a, depth + 1));
                    continue;
                }
                let a = &dec.answers;
                verdicts.push((
                    ChunkVerdict {
                        location: c.label(),
                        p_correct: 1.0 - a["defect"].value,
                        severity: a["severity"].choice.clone().unwrap_or_default(),
                        p_severe: p_severe(&a["severity"]),
                        p_security_issue: a["security"].value,
                        p_unfinished: a["unfinished"].value,
                        p_reviewer_injection: a["reviewer_text"].value,
                        quality: round2(a["quality"].value / 4.0 * 10.0),
                        truncated: dec.truncated,
                    },
                    a["defect"].confidence,
                ));
                weights.push(c.text.len() as f64);
            }
        }
    }

    // Completeness: on the code itself when it fits in one chunk, else on a digest.
    let (text, basis) = if chunks.len() == 1 {
        (chunks[0].text.clone(), "code")
    } else {
        (digest(files, cfg.chunk_chars), "digest")
    };
    let complete_state = match completeness_state(task, basis, text) {
        Ok(s) => s,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    let p_complete = match deadline
        .bound(
            d.decide(&complete_state, &completeness_spec(basis).wire()),
            verdicts.len(),
            chunks.len(),
        )
        .await
    {
        Err(e) => {
            r.error = Some(e);
            return r;
        }
        Ok(dec) => {
            r.latency_ms += dec.latency_ms;
            dec.answers["complete"].value
        }
    };

    aggregate(&mut r, &verdicts, &weights, p_complete, basis);
    r
}

/// The whole pass's time limit. Each request has its own (`kev_request_s`); this bounds the sum,
/// so a pass that slows to a crawl reports how far it got instead of running for hours.
pub struct Deadline {
    at: std::time::Instant,
    limit_s: u64,
}

impl Deadline {
    pub fn new(cfg: &Config) -> Self {
        Deadline {
            at: std::time::Instant::now() + std::time::Duration::from_secs(cfg.critic_deadline_s),
            limit_s: cfg.critic_deadline_s,
        }
    }

    pub fn passed(&self, done: usize, total: usize) -> Option<String> {
        (std::time::Instant::now() >= self.at).then(|| {
            format!(
                "critic stalled: past its {}s deadline with {done} of {total} units scored",
                self.limit_s
            )
        })
    }

    /// Cancel this request future at the shared pass deadline, including an
    /// in-flight grouped decision. Dropping HTTP work does not prove a remote
    /// inference stopped; no shared service is killed here.
    pub async fn bound<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, String>>,
        done: usize,
        total: usize,
    ) -> Result<T, String> {
        tokio::time::timeout_at(self.at.into(), future)
            .await
            .map_err(|_| format!(
                "critic deadline exceeded: {}s; {done}/{total} units complete; request future cancelled; remote cancellation unconfirmed",
                self.limit_s
            ))?
    }
}

fn p_severe(a: &crate::kev::Answer) -> f64 {
    a.probabilities
        .as_ref()
        .map(|p| p.get("major").copied().unwrap_or(0.0) + p.get("critical").copied().unwrap_or(0.0))
        .unwrap_or(0.0)
}

/// Split a hunk's body in two, keeping the @@ header on both halves.
fn halve_hunk(h: &Hunk) -> Option<(Hunk, Hunk)> {
    let mut lines = h.text.lines();
    let header = lines.next()?;
    let body: Vec<&str> = lines.collect();
    if body.len() < 2 {
        return None;
    }
    let mid = body.len() / 2;
    let mk = |part: &[&str]| Hunk {
        file: h.file.clone(),
        new_start: h.new_start,
        text: format!("{header}\n{}\n", part.join("\n")),
    };
    Some((mk(&body[..mid]), mk(&body[mid..])))
}

/// Split a chunk in two on a line boundary (for re-scoring after truncation).
fn halve(c: &Chunk) -> Option<(Chunk, Chunk)> {
    let lines: Vec<&str> = c.text.lines().collect();
    if lines.len() < 2 {
        return None;
    }
    let mid = lines.len() / 2;
    let a = Chunk {
        text: lines[..mid].join("\n") + "\n",
        end_line: c.start_line + mid - 1,
        ..c.clone()
    };
    let b = Chunk {
        text: lines[mid..].join("\n") + "\n",
        start_line: c.start_line + mid,
        ..c.clone()
    };
    Some((a, b))
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Pure aggregation, separated for testing.
pub fn aggregate(
    r: &mut CriticResult,
    verdicts: &[(ChunkVerdict, f64)],
    weights: &[f64],
    p_complete: f64,
    basis: &str,
) {
    r.kev_ok = true;
    r.chunks_scored = verdicts.len();
    r.completeness_basis = basis.into();
    let fmin = |f: fn(&ChunkVerdict) -> f64| {
        verdicts
            .iter()
            .map(|(v, _)| f(v))
            .fold(f64::INFINITY, f64::min)
    };
    let fmax = |f: fn(&ChunkVerdict) -> f64| verdicts.iter().map(|(v, _)| f(v)).fold(0.0, f64::max);
    let p_correct = fmin(|v| v.p_correct);
    let p_sec = fmax(|v| v.p_security_issue);
    let p_unf = fmax(|v| v.p_unfinished);
    let p_inj = fmax(|v| v.p_reviewer_injection);
    let worst = verdicts
        .iter()
        .map(|(v, _)| v.severity.as_str())
        .max_by_key(|s| severity_rank(s))
        .unwrap_or("none");
    let p_severe_max = verdicts.iter().map(|(v, _)| v.p_severe).fold(0.0, f64::max);
    let wsum: f64 = weights.iter().sum::<f64>().max(1.0);
    let quality = verdicts
        .iter()
        .zip(weights)
        .map(|((v, _), w)| v.quality * w)
        .sum::<f64>()
        / wsum;

    r.p_correct_min = Some(round2(p_correct));
    r.p_complete = Some(round2(p_complete));
    r.p_security_issue_max = Some(round2(p_sec));
    r.p_unfinished_max = Some(round2(p_unf));
    r.p_reviewer_injection_max = Some(round2(p_inj));
    r.worst_severity = Some(worst.to_string());
    r.p_severe_max = Some(round2(p_severe_max));
    r.correctness = Some(round2(p_correct * 10.0));
    r.completeness = Some(round2(p_complete * 10.0));
    r.quality = Some(round2(quality));
    r.confidence = Some(round2(
        verdicts.iter().map(|(_, c)| c).sum::<f64>() / verdicts.len().max(1) as f64,
    ));
    r.truncated_chunks = verdicts.iter().filter(|(v, _)| v.truncated).count();

    // Approval comes only from the specs' calibrated gates. Questions without calibration
    // (defect, severity, quality, completeness on a digest) are reported, never decide.
    // The change fails if its worst unit crosses a gate, so the per-unit threshold is corrected
    // for the number of units (see spec::Gate::Rule::good_tail).
    let unit = unit_spec(&r.mode);
    let n = verdicts.len().max(1);
    let per_chunk = |v: &ChunkVerdict| -> Vec<String> {
        [
            ("defect", 1.0 - v.p_correct),
            ("security", v.p_security_issue),
            ("unfinished", v.p_unfinished),
            ("reviewer_text", v.p_reviewer_injection),
        ]
        .into_iter()
        .filter_map(|(id, value)| unit.check_units(id, value, n))
        .collect()
    };
    r.flagged_chunks = verdicts
        .iter()
        .filter(|(v, _)| !per_chunk(v).is_empty())
        .map(|(v, _)| v.clone())
        .collect();
    r.advisory_chunks = verdicts
        .iter()
        .filter(|(v, _)| severity_rank(&v.severity) >= severity_rank("major"))
        .map(|(v, _)| v.clone())
        .collect();
    r.advisory_notes.clear();
    if basis == "digest" {
        r.advisory_notes.push("Completeness was estimated from a digest, not task execution or the full artifact; it does not gate approval.".into());
    }
    if !r.advisory_chunks.is_empty() {
        r.advisory_notes.push("Major/critical labels are uncalibrated advisory opinions. Inspect advisory_chunks for locations; approval is computed separately from calibrated gates.".into());
    }
    let mut reasons: Vec<String> = [
        ("defect", 1.0 - p_correct),
        ("security", p_sec),
        ("unfinished", p_unf),
        ("reviewer_text", p_inj),
    ]
    .into_iter()
    .filter_map(|(id, value)| unit.check_units(id, value, n))
    .collect();
    let completeness = completeness_spec(basis);
    reasons.extend(completeness.check_value("complete", p_complete));
    r.gates = unit
        .questions
        .iter()
        .filter(|q| unit.gates(&q.id))
        .map(|q| format!("{}.{}", unit.name, q.id))
        .collect();
    if completeness.gates("complete") {
        r.gates.push(format!("{}.complete", completeness.name));
    }
    // Rule failures reject; coverage gaps only make the verdict unknown.
    let rules_pass = reasons.is_empty();
    let mut coverage = Vec::new();
    // Under a model the gates were not calibrated for, the probabilities are reported but no
    // verdict is claimed: an uncalibrated "approved" would be a guess presented as a result.
    if let Some(note) = unit.uncalibrated_note() {
        r.gates.clear();
        coverage.push(note);
    }
    if r.chunks_unscored > 0 {
        coverage.push(format!("{} chunks over the {MAX_CHUNKS}-chunk limit were not scored; score file groups separately", r.chunks_unscored));
    }
    if r.truncated_chunks > 0 {
        coverage.push(format!(
            "{} chunks were truncated by the 2,048-token limit",
            r.truncated_chunks
        ));
    }
    r.approval = match (rules_pass, coverage.is_empty()) {
        (false, _) => Some(false),
        (true, false) => None,
        (true, true) => Some(true),
    };
    reasons.extend(coverage);
    r.reasons = reasons;
}

/// State helper used by other tools.
pub fn state_with(task: &str, key: &str, value: Value) -> Value {
    json!({"task": task, key: value})
}

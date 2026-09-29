//! Split artifacts into Kev-sized chunks on line boundaries, and build a compact digest of a
//! multi-file change for task-level questions.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub file: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub text: String,
}

impl Chunk {
    pub fn label(&self) -> String {
        format!("{}:{}-{}", self.file, self.start_line, self.end_line)
    }
}

/// Chunk `text` into pieces of at most `max_chars`, splitting on line boundaries. Prefers to cut
/// at a blank line once a chunk is past 60% full. Over-long single lines are hard-split.
pub fn chunk_text(file: &str, text: &str, max_chars: usize) -> Vec<Chunk> {
    let max_chars = max_chars.max(200);
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len().max(1);
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut start = 1usize;
    let flush = |buf: &mut String, start: &mut usize, end: usize, out: &mut Vec<Chunk>| {
        if !buf.trim().is_empty() {
            out.push(Chunk { file: file.into(), start_line: *start, end_line: end, total_lines: total, text: std::mem::take(buf) });
        } else {
            buf.clear();
        }
        *start = end + 1;
    };
    for (i, line) in lines.iter().enumerate() {
        let n = i + 1;
        if line.chars().count() > max_chars {
            flush(&mut buf, &mut start, n - 1, &mut out);
            let chars: Vec<char> = line.chars().collect();
            for piece in chars.chunks(max_chars) {
                out.push(Chunk { file: file.into(), start_line: n, end_line: n, total_lines: total, text: piece.iter().collect() });
            }
            start = n + 1;
            continue;
        }
        if buf.len() + line.len() + 1 > max_chars {
            flush(&mut buf, &mut start, n - 1, &mut out);
        }
        buf.push_str(line);
        buf.push('\n');
        if line.trim().is_empty() && buf.len() * 10 >= max_chars * 6 {
            flush(&mut buf, &mut start, n, &mut out);
        }
    }
    flush(&mut buf, &mut start, lines.len(), &mut out);
    out
}

fn def_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^\s*(?:pub(?:\([^)]*\))?\s+|export\s+(?:default\s+)?|async\s+|static\s+|public\s+|private\s+)*(?:fn|def|class|function|interface|type|struct|enum|trait|impl|const\s+\w+\s*=\s*(?:async\s*)?\(|let\s+\w+\s*=\s*(?:async\s*)?\()\b.*",
        )
        .unwrap()
    })
}

/// File list + top-level definition lines, clipped to `max_chars`. Used for task-level
/// completeness when the whole change doesn't fit in one Kev sequence.
pub fn digest(files: &[(String, String)], max_chars: usize) -> String {
    let mut out = String::new();
    for (path, text) in files {
        let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(path.clone());
        out.push_str(&format!("FILE {name} ({} lines)\n", text.lines().count()));
        // Definitions plus the first line of each doc-comment block, so Kev sees intent, not just names.
        let mut prev_doc = false;
        let keep = |l: &str, prev_doc: &mut bool| {
            let t = l.trim_start();
            let doc = t.starts_with("//!") || t.starts_with("///") || t.starts_with("\"\"\"") || t.starts_with("/**");
            let first_doc = doc && !*prev_doc;
            *prev_doc = doc;
            first_doc || def_regex().is_match(l)
        };
        for line in text.lines().filter(|l| keep(l, &mut prev_doc)) {
            let l = line.trim();
            let l: String = l.chars().take(120).collect();
            out.push_str("  ");
            out.push_str(&l);
            out.push('\n');
            if out.len() > max_chars {
                break;
            }
        }
        if out.len() > max_chars {
            break;
        }
    }
    if out.len() > max_chars {
        let cut: String = out.chars().take(max_chars).collect();
        return format!("{cut}\n[digest truncated]");
    }
    out
}

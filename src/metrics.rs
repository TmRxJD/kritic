//! Language-agnostic code metrics computed from source text: function length, a cyclomatic
//! complexity approximation (1 + branch tokens), nesting depth, and duplicated blocks.
//! Heuristic but deterministic; exact per-language tools (clippy, ruff C901, eslint complexity)
//! run alongside when the project has them.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct FunctionMetric {
    pub file: String,
    pub name: String,
    pub start_line: usize,
    pub lines: usize,
    pub complexity: usize,
    pub max_nesting: usize,
    #[serde(skip)]
    pub body: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Duplicate {
    pub first: String,
    pub second: String,
    pub lines: usize,
}

fn fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^\s*(?:(?:pub(?:\([^)]*\))?|export|default|async|static|public|private|protected|override|const|unsafe)\s+)*(?:fn|def|function|func)\s+([A-Za-z_][A-Za-z0-9_]*)|^\s*(?:const|let|var)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_]\w*)\s*=>",
        )
        .unwrap()
    })
}

fn branch_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b(if|elif|for|while|case|catch|except|when)\b|&&|\|\||\?\s*[^:?]+:|=>")
            .unwrap()
    })
}

fn strip_strings(line: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`[^`]*`|//.*$|#.*$"#).unwrap()
    });
    re.replace_all(line, "\"\"").into_owned()
}

fn indent(l: &str) -> usize {
    l.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Find functions and measure them. Python-like files use indentation; others use braces.
pub fn functions(file: &str, text: &str) -> Vec<FunctionMetric> {
    let lines: Vec<&str> = text.lines().collect();
    let python = file.ends_with(".py");
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let Some(c) = fn_re().captures(l) else {
            continue;
        };
        let name = c
            .get(1)
            .or(c.get(2))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let end = if python {
            let base = indent(l);
            let mut j = i + 1;
            while j < lines.len() && (lines[j].trim().is_empty() || indent(lines[j]) > base) {
                j += 1;
            }
            j
        } else {
            let mut depth: i64 = 0;
            let mut opened = false;
            let mut j = i;
            while j < lines.len() {
                let s = strip_strings(lines[j]);
                for ch in s.chars() {
                    if ch == '{' {
                        depth += 1;
                        opened = true;
                    } else if ch == '}' {
                        depth -= 1;
                    }
                }
                j += 1;
                if opened && depth <= 0 {
                    break;
                }
                if !opened && j - i > 3 {
                    break; // declaration without a body (trait fn, prototype)
                }
            }
            if !opened {
                continue;
            }
            j
        };
        let body = &lines[i..end.min(lines.len())];
        let complexity = 1 + body
            .iter()
            .map(|b| branch_re().find_iter(&strip_strings(b)).count())
            .sum::<usize>();
        let base_indent = indent(l);
        let max_nesting = body
            .iter()
            .filter(|b| !b.trim().is_empty())
            .map(|b| indent(b).saturating_sub(base_indent) / 4)
            .max()
            .unwrap_or(0);
        out.push(FunctionMetric {
            file: file.into(),
            name,
            start_line: i + 1,
            lines: body.len(),
            complexity,
            max_nesting,
            body: body.join("\n"),
        });
    }
    out
}

/// Blocks of >= `window` identical normalized lines appearing in two places.
pub fn duplicates(files: &[(String, String)], window: usize) -> Vec<Duplicate> {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut out: Vec<Duplicate> = Vec::new();
    for (f, text) in files {
        let norm: Vec<(usize, String)> = text
            .lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.trim().to_string()))
            .filter(|(_, l)| {
                l.len() > 3
                    && !l.starts_with("//")
                    && !l.starts_with('#')
                    && !matches!(l.as_str(), "}" | "{" | "};" | ")" | "]")
            })
            .collect();
        let mut i = 0;
        while i + window <= norm.len() {
            let key: String = norm[i..i + window]
                .iter()
                .map(|(_, l)| l.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let here = format!("{f}:{}", norm[i].0);
            if let Some(first) = seen.get(&key) {
                out.push(Duplicate {
                    first: first.clone(),
                    second: here,
                    lines: window,
                });
                i += window; // don't report overlapping windows of the same block
                continue;
            }
            seen.insert(key, here);
            i += 1;
        }
        if out.len() >= 20 {
            break;
        }
    }
    out
}

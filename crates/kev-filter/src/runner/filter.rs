//! Bounded line framing and extractive selection. Nothing here establishes command success.
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Debug)]
pub struct Record {
    pub stream: usize,
    pub line: u64,
    pub offset: u64,
    pub text: String,
    pub lossy: bool,
}
impl Record {
    pub fn json(&self, why: &str) -> Value {
        json!({"stream":if self.stream==0{"stdout"}else{"stderr"},"line":self.line,"byte_offset":self.offset,"text":self.text,"lossy":self.lossy,"reason":why})
    }
}

#[derive(Default)]
pub struct Framer {
    pending: Vec<u8>,
    offset: u64,
    line: u64,
}
impl Framer {
    pub fn feed(&mut self, stream: usize, bytes: &[u8], eof: bool) -> Vec<Record> {
        self.pending.extend_from_slice(bytes);
        let mut result = Vec::new();
        loop {
            let delimiter = self.pending.iter().position(|b| matches!(b, b'\n' | b'\r'));
            let (end, newline) = match delimiter {
                Some(i) => (i + 1, self.pending[i] == b'\n'),
                None if self.pending.len() >= 4096 => {
                    let end = match std::str::from_utf8(&self.pending[..4096]) {
                        Err(e) if e.error_len().is_none() => e.valid_up_to(),
                        _ => 4096,
                    };
                    (end, false)
                }
                None if eof && !self.pending.is_empty() => (self.pending.len(), false),
                _ => break,
            };
            let bytes: Vec<_> = self.pending.drain(..end).collect();
            let text = String::from_utf8_lossy(&bytes);
            result.push(Record {
                stream,
                line: self.line + 1,
                offset: self.offset,
                text: sanitize(&text),
                lossy: std::str::from_utf8(&bytes).is_err(),
            });
            self.offset += end as u64;
            if newline {
                self.line += 1;
            }
        }
        result
    }
}

/// Strip terminal control sequences in the display only; raw archives remain byte-for-byte.
pub fn sanitize(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let mut escape = false;
                    for next in chars.by_ref() {
                        if next == '\u{7}' || (escape && next == '\\') {
                            break;
                        }
                        escape = next == '\u{1b}';
                    }
                }
                _ => {}
            }
        } else if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out
}

pub fn diagnostic(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "error",
        "warn",
        "fail",
        "panic",
        "fatal",
        "traceback",
        "exception",
        "denied",
        "not found",
        "undefined",
        "timed out",
        "password",
        "confirm",
        "press ",
        "[y/n]",
    ]
    .iter()
    .any(|word| text.contains(word))
}
pub fn noise(text: &str, cargo: bool) -> bool {
    let text = text.trim();
    text.is_empty()
        || (cargo
            && (text.starts_with("Compiling ")
                || text.starts_with("Checking ")
                || (text.starts_with("test ") && text.ends_with(" ... ok"))))
        || (text.contains('%')
            && text
                .chars()
                .all(|c| c.is_ascii_digit() || " %./\\|-=[]>#".contains(c)))
}

pub struct Selection {
    pub records: Vec<Value>,
    pub chars: usize,
    pub omitted: u64,
    pub repeated: u64,
    pub noise: u64,
    pub ranked_out: u64,
    pub clipped: bool,
    budget: usize,
    json: bool,
    cargo: bool,
    emitted: HashSet<(usize, u64)>,
    seen: HashSet<String>,
    pub tail: VecDeque<Record>,
    context: [usize; 2],
}
impl Selection {
    pub fn new(budget: usize, json: bool, cargo: bool) -> Self {
        Self {
            records: vec![],
            chars: 0,
            omitted: 0,
            repeated: 0,
            noise: 0,
            ranked_out: 0,
            clipped: false,
            budget,
            json,
            cargo,
            emitted: HashSet::new(),
            seen: HashSet::new(),
            tail: VecDeque::new(),
            context: [0; 2],
        }
    }
    /// Returns true only for an ordinary stdout candidate which Kev may rank.
    pub fn observe(&mut self, record: &Record) -> bool {
        self.tail.push_back(record.clone());
        if self.tail.len() > 32 {
            self.tail.pop_front();
        }
        if diagnostic(&record.text) {
            self.context[record.stream] = 2;
            if self.seen.contains(&record.text) {
                self.repeated += 1;
                return false;
            }
            if self.seen.len() < 4096 {
                self.seen.insert(record.text.clone());
            }
            let before = self
                .tail
                .iter()
                .rev()
                .skip(1)
                .filter(|r| r.stream == record.stream)
                .take(2)
                .cloned()
                .collect::<Vec<_>>();
            for context in before.iter().rev() {
                self.keep(context, "diagnostic context", true);
            }
            self.keep(record, "diagnostic", true);
            return false;
        }
        if self.context[record.stream] > 0 {
            self.context[record.stream] -= 1;
            self.keep(record, "diagnostic context", true);
            return false;
        }
        if noise(&record.text, self.cargo) {
            self.noise += 1;
            return false;
        }
        if self.seen.contains(&record.text) {
            self.repeated += 1;
            return false;
        }
        if self.seen.len() < 4096 {
            self.seen.insert(record.text.clone());
        }
        if record.stream == 1 || record.line <= 3 {
            self.keep(
                record,
                if record.stream == 1 {
                    "stderr"
                } else {
                    "opening"
                },
                false,
            );
            return false;
        }
        true
    }
    pub fn keep(&mut self, record: &Record, why: &str, closing: bool) {
        if self.emitted.contains(&(record.stream, record.offset)) {
            return;
        }
        let limit = if closing {
            self.budget
        } else {
            self.budget * 3 / 4
        };
        let available = limit.saturating_sub(self.chars);
        if available == 0 {
            self.omitted += 1;
            self.clipped = true;
            return;
        }
        let mut shown = record.clone();
        let size = shown.text.chars().count();
        if size > available {
            shown.text = shown.text.chars().take(available).collect();
            self.clipped = true;
        }
        self.chars += shown.text.chars().count();
        self.emitted.insert((record.stream, record.offset));
        if !self.json {
            println!(
                "[{}:{}@{}] {}",
                if record.stream == 0 {
                    "stdout"
                } else {
                    "stderr"
                },
                record.line,
                record.offset,
                shown.text
            );
        }
        let mut value = shown.json(why);
        value["excerpt_clipped"] = json!(size > available);
        self.records.push(value);
    }
    pub fn closing(&mut self) {
        for record in self.tail.clone().iter().rev().take(8).rev() {
            self.keep(record, "closing context", true);
        }
    }
}

//! Beat Saber difficulty evaluation: deterministic stats + parity/grid heuristics decide; Kev can
//! optionally read 16-beat slices (spec `map_slice`, advisory).
//!
//! Kev is not asked about the stats. Measured 2026-09-27 on ranked maps vs broken fixtures, its
//! judgments of raw stats did not separate them (random notes scored as playable as ranked maps),
//! and once the stats are banded by code the verdict is already decided, so asking adds nothing.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;

use crate::checks::Check;
use crate::config::Config;
use crate::kev::Decider;

pub const SLICE_BEATS: f64 = 16.0;
/// A same-direction swing after this many beats is a reset, not a parity break (the common
/// community validators use the same window).
pub const PARITY_RESET_BEATS: f64 = 1.5;
/// Parity breaks per note above which a map fails. Ranked maps measured with this rule reach
/// 0.063 (111 top ranked difficulties), so 5% failed real ranked maps.
pub const PARITY_BREAK_RATE_MAX: f64 = 0.08;
pub const MAX_SLICES: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub beat: f64,
    pub x: i64,
    pub y: i64,
    /// 0 red (left hand), 1 blue (right hand)
    pub color: i64,
    /// 0 up, 1 down, 2 left, 3 right, 4 up-left, 5 up-right, 6 down-left, 7 down-right, 8 any
    pub dir: i64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct MapStats {
    pub format: String,
    pub note_count: usize,
    pub bomb_count: usize,
    pub obstacle_count: usize,
    pub beat_span: f64,
    pub notes_per_beat: Option<f64>,
    pub peak_notes_per_4_beats: usize,
    pub out_of_grid: usize,
    pub unsorted_pairs: usize,
    pub duplicate_positions: usize,
    pub invalid_fields: usize,
    pub invalid_directions: usize,
    /// Same hand swinging the same way twice in a row (down-down / up-up) within
    /// `PARITY_RESET_BEATS`, with no sideways or "any" note between. A longer gap is a reset.
    pub parity_breaks: usize,
    /// parity_breaks / note_count.
    pub parity_break_rate: Option<f64>,
    pub red_blue_ratio: Option<f64>,
    /// Notes per 8-beat window, first 64 windows.
    pub density_per_8_beats: Vec<usize>,
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

/// Parse a v2 (`_notes`) or v3 (`colorNotes`) difficulty. Returns None when neither key exists.
pub fn parse_notes(doc: &Value) -> Option<(String, Vec<Note>, usize, usize, usize)> {
    // v4 keeps positions in a shared table: colorNotes [{b, i}] index colorNotesData [{x, y, c, d}].
    if let (Some(refs), Some(data)) = (
        doc.get("colorNotes").and_then(Value::as_array),
        doc.get("colorNotesData").and_then(Value::as_array),
    ) {
        let mut notes = Vec::new();
        let mut invalid = 0;
        for n in refs {
            // v4 omits zero-valued fields, so a missing b/i/x/y/c/d is 0, not an error;
            // only an index past the end of colorNotesData is invalid.
            let index = n.get("i").map_or(Some(0), Value::as_u64);
            let b = num(n.get("b")).unwrap_or(0.0);
            match index.and_then(|i| data.get(i as usize)) {
                Some(d) => {
                    let g = |k: &str| num(d.get(k)).unwrap_or(0.0) as i64;
                    notes.push(Note {
                        beat: b,
                        x: g("x"),
                        y: g("y"),
                        color: g("c"),
                        dir: g("d"),
                    });
                }
                _ => invalid += 1,
            }
        }
        let bombs = doc
            .get("bombNotes")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let obstacles = doc
            .get("obstacles")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        return Some(("v4".into(), notes, bombs, obstacles, invalid));
    }
    let (fmt, arr, keys) = if let Some(a) = doc.get("colorNotes").and_then(Value::as_array) {
        ("v3", a, ("b", "x", "y", "c", "d"))
    } else {
        let a = doc.get("_notes").and_then(Value::as_array)?;
        (
            "v2",
            a,
            (
                "_time",
                "_lineIndex",
                "_lineLayer",
                "_type",
                "_cutDirection",
            ),
        )
    };
    let mut notes = Vec::new();
    let mut invalid = 0;
    let mut bombs = 0;
    for n in arr {
        let get = |k: &str| num(n.get(k));
        match (
            get(keys.0),
            get(keys.1),
            get(keys.2),
            get(keys.3),
            get(keys.4),
        ) {
            (Some(b), Some(x), Some(y), Some(c), Some(d)) => {
                if fmt == "v2" && c as i64 == 3 {
                    bombs += 1;
                    continue;
                }
                notes.push(Note {
                    beat: b,
                    x: x as i64,
                    y: y as i64,
                    color: c as i64,
                    dir: d as i64,
                });
            }
            // v3 notes may omit zero-valued fields in some editors; treat a missing field as invalid
            _ => invalid += 1,
        }
    }
    if fmt == "v3" {
        bombs = doc
            .get("bombNotes")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
    }
    let obstacles = doc
        .get("obstacles")
        .or_else(|| doc.get("_obstacles"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Some((fmt.into(), notes, bombs, obstacles, invalid))
}

fn swing(dir: i64) -> i8 {
    match dir {
        0 | 4 | 5 => 1,  // up
        1 | 6 | 7 => -1, // down
        _ => 0,          // sideways / any
    }
}

pub fn stats(
    fmt: &str,
    notes: &[Note],
    bombs: usize,
    obstacles: usize,
    invalid: usize,
) -> MapStats {
    let mut s = MapStats {
        format: fmt.into(),
        note_count: notes.len(),
        bomb_count: bombs,
        obstacle_count: obstacles,
        invalid_fields: invalid,
        ..Default::default()
    };
    if notes.is_empty() {
        return s;
    }
    let (lo, hi) = notes
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), n| {
            (a.min(n.beat), b.max(n.beat))
        });
    s.beat_span = hi - lo;
    s.notes_per_beat =
        (s.beat_span > 0.0).then(|| ((notes.len() as f64 / s.beat_span) * 1000.0).round() / 1000.0);
    s.out_of_grid = notes
        .iter()
        .filter(|n| !(0..=3).contains(&n.x) || !(0..=2).contains(&n.y))
        .count();
    s.invalid_directions = notes.iter().filter(|n| !(0..=8).contains(&n.dir)).count();
    s.unsorted_pairs = notes.windows(2).filter(|w| w[1].beat < w[0].beat).count();
    let mut seen = HashSet::new();
    s.duplicate_positions = notes
        .iter()
        .filter(|n| !seen.insert((n.beat.to_bits(), n.x, n.y)))
        .count();
    let mut sorted = notes.to_vec();
    sorted.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    for hand in [0, 1] {
        let mut last: i8 = 0;
        let mut last_beat = f64::NEG_INFINITY;
        for n in sorted.iter().filter(|n| n.color == hand) {
            let sw = swing(n.dir);
            // notes on the same beat for one hand are a stack/slider, not a new swing
            if (n.beat - last_beat).abs() < 1e-3 {
                continue;
            }
            if sw != 0 && sw == last && n.beat - last_beat < PARITY_RESET_BEATS {
                s.parity_breaks += 1;
            }
            last = if n.dir == 8 { 0 } else { sw };
            last_beat = n.beat;
        }
    }
    s.parity_break_rate =
        Some(((s.parity_breaks as f64 / notes.len() as f64) * 1000.0).round() / 1000.0);
    let red = notes.iter().filter(|n| n.color == 0).count();
    let blue = notes.iter().filter(|n| n.color == 1).count();
    s.red_blue_ratio = (blue > 0).then(|| ((red as f64 / blue as f64) * 100.0).round() / 100.0);
    let win = |w: f64| {
        let n = ((s.beat_span / w).floor() as usize) + 1;
        let mut v = vec![0usize; n];
        for note in notes {
            let i = ((note.beat - lo) / w).floor() as usize;
            if i < n {
                v[i] += 1;
            }
        }
        v
    };
    s.peak_notes_per_4_beats = win(4.0).into_iter().max().unwrap_or(0);
    s.density_per_8_beats = win(8.0).into_iter().take(64).collect();
    s
}

pub fn heuristic_issues(s: &MapStats) -> Vec<String> {
    let mut v = Vec::new();
    for (n, label) in [
        (s.out_of_grid, "notes outside the 4x3 grid"),
        (s.unsorted_pairs, "notes out of time order"),
        (s.duplicate_positions, "notes share time+position"),
        (s.invalid_fields, "notes with missing/non-numeric fields"),
        (s.invalid_directions, "notes with invalid cut direction"),
    ] {
        if n > 0 {
            v.push(format!("{n} {label}"));
        }
    }
    if s.note_count > 0 && (s.parity_breaks as f64) > PARITY_BREAK_RATE_MAX * s.note_count as f64 {
        v.push(format!(
            "{} parity breaks ({:.1}% of notes > {:.0}%)",
            s.parity_breaks,
            100.0 * s.parity_breaks as f64 / s.note_count as f64,
            PARITY_BREAK_RATE_MAX * 100.0
        ));
    }
    v
}

const DIRS: [&str; 9] = [
    "up",
    "down",
    "left",
    "right",
    "upleft",
    "upright",
    "downleft",
    "downright",
    "any",
];

/// Compact text slices of `SLICE_BEATS` beats, each at most `max_chars`.
pub fn slices(notes: &[Note], max_chars: usize) -> Vec<(f64, f64, String)> {
    let mut sorted = notes.to_vec();
    sorted.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    let mut out: Vec<(f64, f64, String)> = Vec::new();
    for n in &sorted {
        let start = (n.beat / SLICE_BEATS).floor() * SLICE_BEATS;
        let hand = if n.color == 0 { "L" } else { "R" };
        let dir = DIRS.get(n.dir as usize).copied().unwrap_or("?");
        let line = format!("{:.2} {hand} x{} y{} {dir}\n", n.beat, n.x, n.y);
        match out.last_mut() {
            Some((s, _, t)) if *s == start && t.len() + line.len() <= max_chars => {
                t.push_str(&line)
            }
            _ => out.push((start, start + SLICE_BEATS, line)),
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SliceVerdict {
    pub start_beat: f64,
    pub end_beat: f64,
    pub p_awkward: f64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema, Default)]
pub struct MapResult {
    pub tool: String,
    pub advisory: bool,
    pub model: String,
    pub kev_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub map_kind: String,
    pub deterministic: Vec<Check>,
    pub stats: Option<MapStats>,
    /// Deterministic rule failures. Any entry forces approval=false.
    pub heuristic_issues: Vec<String>,
    /// Max p(awkward) over the scored slices (spec `map_slice`, advisory), when `kev_slices`.
    pub p_awkward_max: Option<f64>,
    pub flagged_slices: Vec<SliceVerdict>,
    pub slices_scored: usize,
    pub slices_unscored: usize,
    pub approval: Option<bool>,
    pub reasons: Vec<String>,
    /// Kev's opinions that did not decide approval (see `evaluate`).
    pub kev_advisory: Vec<String>,
    pub latency_ms: u64,
}

pub async fn evaluate<D: Decider>(
    d: &D,
    cfg: &Config,
    content: &str,
    bpm: Option<f64>,
    kev_slices: bool,
) -> MapResult {
    let mut r = MapResult {
        tool: "map_eval".into(),
        advisory: true,
        model: cfg.model.clone(),
        map_kind: "unknown".into(),
        ..Default::default()
    };
    let doc: Value = match serde_json::from_str(content) {
        Ok(v) => {
            r.deterministic.push(Check {
                check: "json_parse".into(),
                status: crate::checks::Status::Pass,
                detail: None,
            });
            v
        }
        Err(e) => {
            r.deterministic.push(Check {
                check: "json_parse".into(),
                status: crate::checks::Status::Fail,
                detail: Some(e.to_string()),
            });
            r.approval = Some(false);
            r.reasons.push("map is not valid JSON".into());
            return r;
        }
    };
    let Some((fmt, notes, bombs, obstacles, invalid)) = parse_notes(&doc) else {
        r.map_kind = "generic".into();
        r.reasons.push("not a Beat Saber difficulty (no colorNotes or _notes): only the JSON parse was checked".into());
        r.approval = None;
        return r;
    };
    r.map_kind = "beat_saber".into();
    let s = stats(&fmt, &notes, bombs, obstacles, invalid);
    r.heuristic_issues = heuristic_issues(&s);
    r.stats = Some(s);
    r.reasons = r.heuristic_issues.clone();
    r.approval = Some(r.reasons.is_empty());
    r.kev_ok = true;
    if !kev_slices {
        return r;
    }

    let spec = crate::spec::get("map_slice");
    let q = spec.wire();
    let mut all = slices(&notes, cfg.chunk_chars);
    if all.len() > MAX_SLICES {
        // Sample evenly across the song rather than scoring only its opening.
        r.slices_unscored = all.len() - MAX_SLICES;
        let step = all.len() as f64 / MAX_SLICES as f64;
        all = (0..MAX_SLICES)
            .map(|i| all[(i as f64 * step) as usize].clone())
            .collect();
    }
    let mut p_max: f64 = 0.0;
    for (a, b, text) in &all {
        let count = text.lines().count();
        let tempo = match bpm {
            Some(bpm) => format!(
                "{bpm:.0} BPM; {:.1} notes per second in this stretch",
                count as f64 / (SLICE_BEATS * 60.0 / bpm)
            ),
            None => format!("unknown BPM; {count} notes in these {SLICE_BEATS:.0} beats"),
        };
        let state = match spec.state(&[("notes", text.trim_end().to_string()), ("tempo", tempo)]) {
            Ok(s) => s,
            Err(e) => {
                r.kev_advisory
                    .push(format!("slice {a}-{b} not scored: {e}"));
                continue;
            }
        };
        match d.decide(&state, &q).await {
            Err(e) => {
                r.kev_ok = false;
                r.error = Some(e);
                return r;
            }
            Ok(dec) => {
                r.latency_ms += dec.latency_ms;
                let p = dec.answers["awkward"].value;
                p_max = p_max.max(p);
                if p > 0.5 {
                    r.flagged_slices.push(SliceVerdict {
                        start_beat: *a,
                        end_beat: *b,
                        p_awkward: p,
                    });
                }
            }
        }
    }
    r.slices_scored = all.len();
    r.p_awkward_max = Some(p_max);
    if !r.flagged_slices.is_empty() {
        r.kev_advisory.push(format!("{} slices Kev leans awkward on (p > 0.5); advisory: map_slice.awkward measured AUC 0.685", r.flagged_slices.len()));
    }
    if r.slices_unscored > 0 {
        r.kev_advisory.push(format!(
            "{} of {} slices sampled",
            MAX_SLICES,
            MAX_SLICES + r.slices_unscored
        ));
    }
    r
}

/// The song's BPM from the Info.dat beside a difficulty (v2 `_beatsPerMinute`, v4 `audio.bpm`).
pub fn bpm_beside(map_path: &std::path::Path) -> Option<f64> {
    let dir = map_path.parent()?;
    let info = ["Info.dat", "info.dat"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())?;
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(info).ok()?).ok()?;
    doc.get("_beatsPerMinute")
        .and_then(Value::as_f64)
        .or_else(|| doc.pointer("/audio/bpm").and_then(Value::as_f64))
        .filter(|b| *b > 0.0)
}

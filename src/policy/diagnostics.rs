//! Trajectory diagnostics over recorded state arrays: behavior families by local statistics,
//! stuck states, oscillation, unrecoverable metrics, and the fail-fast family veto.
//! Everything is computed in code from the game's thresholds; no model is involved.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::schema::GameSchema;

/// Time segments per dimension in a signature (distribution over time).
pub const SEGMENTS: usize = 4;

/// One episode's states over time (each state is the flat decoded vector).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Trajectory {
    pub seed: u64,
    pub states: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
pub struct StuckSpan {
    /// First tick index (into the trajectory) of the stuck stretch.
    pub start: usize,
    /// One past the last tick index.
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
pub struct OscillationSpan {
    pub field: String,
    pub start: usize,
    pub end: usize,
    pub reversals: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
pub struct UnrecoverableHit {
    pub field: String,
    /// Tick index from which the metric stayed past its limit to the end.
    pub since: usize,
    pub final_value: f64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EpisodeDiag {
    pub seed: u64,
    pub ticks: usize,
    pub family: usize,
    pub progress_start: f64,
    pub progress_max: f64,
    pub made_progress: bool,
    pub success: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stuck: Vec<StuckSpan>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub oscillations: Vec<OscillationSpan>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unrecoverable: Vec<UnrecoverableHit>,
}

/// Per-dimension statistics in the game's own units.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DimSignature {
    pub field: String,
    pub mean: f64,
    pub std: f64,
    /// Mean over each quarter of the episode, in order.
    pub segments: [f64; SEGMENTS],
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Family {
    pub id: usize,
    pub seeds: Vec<u64>,
    pub size: usize,
    pub success_rate: f64,
    pub progress_rate: f64,
    pub mean_progress_max: f64,
    pub stuck_episodes: usize,
    pub oscillating_episodes: usize,
    pub unrecoverable_episodes: usize,
    /// Centroid signature in the game's units.
    pub signature: Vec<DimSignature>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct VetoedFamily {
    pub family: usize,
    pub seeds: Vec<u64>,
    /// `no_progress` (progress never rose), `no_success`, or `below_min_success`.
    pub reason: String,
    pub success_rate: f64,
    pub progress_rate: f64,
    /// Coordinate signature of the family: centroid of normalized [mean, std, segments...] per
    /// dimension, in `coordinate_fields` order.
    pub coordinates: Vec<f64>,
    pub coordinate_fields: Vec<String>,
    pub signature: Vec<DimSignature>,
    /// Dominant issues across the family, most frequent first.
    pub issues: Vec<String>,
}

/// The structured rejection returned to the building agent.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Veto {
    pub rejected: bool,
    pub message: String,
    pub families: Vec<VetoedFamily>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Diagnostics {
    /// false when any family is vetoed; null with no trajectories.
    pub approval: Option<bool>,
    pub veto: Veto,
    pub families: Vec<Family>,
    pub episodes: Vec<EpisodeDiag>,
    /// (lo, hi) per dimension used for normalization (declared, else observed).
    pub ranges: Vec<(f64, f64)>,
}

/// Normalization range per dimension: the schema's declared range, else the observed one.
pub fn ranges(schema: &GameSchema, trajs: &[Trajectory]) -> Vec<(f64, f64)> {
    let declared = schema.declared_ranges();
    (0..schema.dims())
        .map(|d| match declared[d] {
            (Some(lo), Some(hi)) => (lo, hi),
            _ => {
                let mut lo = f64::INFINITY;
                let mut hi = f64::NEG_INFINITY;
                for s in trajs.iter().flat_map(|t| &t.states) {
                    if let Some(v) = s.get(d).filter(|v| v.is_finite()) {
                        lo = lo.min(*v);
                        hi = hi.max(*v);
                    }
                }
                if lo.is_finite() { (lo, hi) } else { (0.0, 1.0) }
            }
        })
        .collect()
}

fn width((lo, hi): (f64, f64)) -> f64 {
    if hi - lo > 0.0 { hi - lo } else { 1.0 }
}

/// Normalized [mean, std, seg0..seg3] per dimension.
pub fn coordinates(t: &Trajectory, rs: &[(f64, f64)]) -> Vec<f64> {
    let n = t.states.len();
    let mut out = Vec::with_capacity(rs.len() * (2 + SEGMENTS));
    for (d, r) in rs.iter().enumerate() {
        let xs: Vec<f64> = t.states.iter().map(|s| (s[d] - r.0) / width(*r)).collect();
        if n == 0 {
            out.extend(std::iter::repeat_n(0.0, 2 + SEGMENTS));
            continue;
        }
        let mean = xs.iter().sum::<f64>() / n as f64;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
        out.push(mean);
        out.push(var.sqrt());
        for k in 0..SEGMENTS {
            let a = k * n / SEGMENTS;
            let b = ((k + 1) * n / SEGMENTS).max(a + 1).min(n);
            let a = a.min(b - 1);
            out.push(xs[a..b].iter().sum::<f64>() / (b - a) as f64);
        }
    }
    out
}

/// Root-mean-square distance between signatures.
pub fn distance(a: &[f64], b: &[f64]) -> f64 {
    if a.is_empty() {
        return 0.0;
    }
    (a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f64>() / a.len() as f64).sqrt()
}

/// Deterministic leader clustering with one reassignment pass. Returns a family id per input,
/// numbered in order of first appearance.
pub fn cluster(coords: &[Vec<f64>], radius: f64) -> Vec<usize> {
    let mut centers: Vec<(Vec<f64>, usize)> = Vec::new();
    for c in coords {
        let near = centers
            .iter()
            .enumerate()
            .map(|(i, (m, _))| (i, distance(m, c)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match near {
            Some((i, d)) if d <= radius => {
                let (m, k) = &mut centers[i];
                *k += 1;
                for (mv, cv) in m.iter_mut().zip(c) {
                    *mv += (cv - *mv) / *k as f64;
                }
            }
            _ => centers.push((c.clone(), 1)),
        }
    }
    let nearest = |c: &Vec<f64>| {
        centers
            .iter()
            .enumerate()
            .min_by(|a, b| distance(&a.1.0, c).total_cmp(&distance(&b.1.0, c)))
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let raw: Vec<usize> = coords.iter().map(nearest).collect();
    let mut order: Vec<usize> = Vec::new();
    raw.iter()
        .map(|r| match order.iter().position(|o| o == r) {
            Some(p) => p,
            None => {
                order.push(*r);
                order.len() - 1
            }
        })
        .collect()
}

fn watched(schema: &GameSchema) -> Vec<usize> {
    let w = &schema.thresholds.watch_fields;
    if w.is_empty() {
        (0..schema.dims()).collect()
    } else {
        w.iter().filter_map(|f| schema.index_of(f).ok()).collect()
    }
}

/// Stretches of at least `stuck_ticks` where every watched value stays within the band.
pub fn stuck_spans(
    t: &Trajectory,
    dims: &[usize],
    rs: &[(f64, f64)],
    ticks: usize,
    eps: f64,
) -> Vec<StuckSpan> {
    let n = t.states.len();
    if ticks == 0 || n < ticks || dims.is_empty() {
        return Vec::new();
    }
    let flat = |a: usize| {
        dims.iter().all(|&d| {
            let w = width(rs[d]);
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for s in &t.states[a..a + ticks] {
                lo = lo.min(s[d]);
                hi = hi.max(s[d]);
            }
            (hi - lo) / w <= eps
        })
    };
    let mut out: Vec<StuckSpan> = Vec::new();
    for a in 0..=n - ticks {
        if flat(a) {
            match out.last_mut() {
                Some(s) if s.end >= a + ticks - 1 => s.end = a + ticks,
                _ => out.push(StuckSpan {
                    start: a,
                    end: a + ticks,
                }),
            }
        }
    }
    out
}

/// Windows where a watched value reverses direction at least `flips` times with steps of at
/// least `amp` (fraction of range).
pub fn oscillation_spans(
    t: &Trajectory,
    dims: &[usize],
    labels: &[String],
    rs: &[(f64, f64)],
    window: usize,
    flips: usize,
    amp: f64,
) -> Vec<OscillationSpan> {
    let mut out = Vec::new();
    for &d in dims {
        let w = width(rs[d]);
        let mut rev: Vec<usize> = Vec::new();
        let mut last_sign = 0i8;
        for (i, pair) in t.states.windows(2).enumerate() {
            let delta = (pair[1][d] - pair[0][d]) / w;
            if delta.abs() < amp {
                continue;
            }
            let sign = if delta > 0.0 { 1 } else { -1 };
            if last_sign != 0 && sign != last_sign {
                rev.push(i + 1);
            }
            last_sign = sign;
        }
        let mut spans: Vec<OscillationSpan> = Vec::new();
        let mut j = 0;
        for i in 0..rev.len() {
            j = j.max(i);
            while j < rev.len() && rev[j] < rev[i] + window {
                j += 1;
            }
            if j - i >= flips {
                let (start, end) = (rev[i].saturating_sub(1), rev[j - 1] + 1);
                match spans.last_mut() {
                    Some(s) if s.end >= start => {
                        s.end = s.end.max(end);
                        s.reversals = rev.iter().filter(|r| **r >= s.start && **r < s.end).count();
                    }
                    _ => spans.push(OscillationSpan {
                        field: labels[d].clone(),
                        start,
                        end,
                        reversals: j - i,
                    }),
                }
            }
        }
        out.extend(spans);
    }
    out
}

/// Metrics past their limit from some tick to the end, for at least `for_ticks`.
pub fn unrecoverable(t: &Trajectory, schema: &GameSchema) -> Vec<UnrecoverableHit> {
    let n = t.states.len();
    let mut out = Vec::new();
    for r in &schema.thresholds.unrecoverable {
        let Ok(d) = schema.index_of(&r.field) else {
            continue;
        };
        let past = |v: f64| r.below.is_some_and(|b| v < b) || r.above.is_some_and(|a| v > a);
        let since = t
            .states
            .iter()
            .rposition(|s| !past(s[d]))
            .map_or(0, |i| i + 1);
        if since < n && n - since >= r.for_ticks.max(1) {
            out.push(UnrecoverableHit {
                field: r.field.clone(),
                since,
                final_value: t.states[n - 1][d],
            });
        }
    }
    out
}

fn denorm(coords: &[f64], rs: &[(f64, f64)], labels: &[String]) -> Vec<DimSignature> {
    let k = 2 + SEGMENTS;
    rs.iter()
        .enumerate()
        .map(|(d, r)| {
            let c = &coords[d * k..(d + 1) * k];
            let w = width(*r);
            let mut segments = [0.0; SEGMENTS];
            for (i, s) in segments.iter_mut().enumerate() {
                *s = round(r.0 + c[2 + i] * w);
            }
            DimSignature {
                field: labels[d].clone(),
                mean: round(r.0 + c[0] * w),
                std: round(c[1] * w),
                segments,
            }
        })
        .collect()
}

fn round(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

/// Full diagnostics for a set of trajectories under the game's thresholds.
pub fn diagnose(schema: &GameSchema, trajs: &[Trajectory]) -> Result<Diagnostics, String> {
    let dims = schema.dims();
    for t in trajs {
        if let Some(s) = t.states.iter().find(|s| s.len() != dims) {
            return Err(format!(
                "seed {}: state has {} values, schema says {dims}",
                t.seed,
                s.len()
            ));
        }
    }
    let th = &schema.thresholds;
    let rs = ranges(schema, trajs);
    let labels = schema.dim_labels();
    let watch = watched(schema);
    let pi = schema.index_of(&th.progress_field)?;
    let coords: Vec<Vec<f64>> = trajs.iter().map(|t| coordinates(t, &rs)).collect();
    let fam = cluster(&coords, th.cluster_radius);
    let mut episodes = Vec::with_capacity(trajs.len());
    for (i, t) in trajs.iter().enumerate() {
        let p0 = t.states.first().map_or(0.0, |s| s[pi]);
        let pmax = t.states.iter().map(|s| s[pi]).fold(p0, f64::max);
        episodes.push(EpisodeDiag {
            seed: t.seed,
            ticks: t.states.len(),
            family: fam[i],
            progress_start: p0,
            progress_max: pmax,
            made_progress: pmax - p0 > th.progress_epsilon,
            success: !t.states.is_empty() && pmax >= th.success_at,
            stuck: stuck_spans(t, &watch, &rs, th.stuck_ticks, th.stuck_epsilon),
            oscillations: oscillation_spans(
                t,
                &watch,
                &labels,
                &rs,
                th.oscillation_window,
                th.oscillation_flips,
                th.oscillation_amplitude,
            ),
            unrecoverable: unrecoverable(t, schema),
        });
    }
    let n_fam = fam.iter().max().map_or(0, |m| m + 1);
    let mut families = Vec::with_capacity(n_fam);
    let mut vetoed = Vec::new();
    let coordinate_fields: Vec<String> = labels
        .iter()
        .flat_map(|l| {
            ["mean", "std"]
                .into_iter()
                .map(String::from)
                .chain((0..SEGMENTS).map(|k| format!("seg{k}")))
                .map(move |s| format!("{l}.{s}"))
        })
        .collect();
    for id in 0..n_fam {
        let members: Vec<usize> = (0..trajs.len()).filter(|i| fam[*i] == id).collect();
        let size = members.len();
        let frac = |f: &dyn Fn(&EpisodeDiag) -> bool| {
            members.iter().filter(|i| f(&episodes[**i])).count() as f64 / size as f64
        };
        let count =
            |f: &dyn Fn(&EpisodeDiag) -> bool| members.iter().filter(|i| f(&episodes[**i])).count();
        let mut centroid = vec![0.0; coords[members[0]].len()];
        for &i in &members {
            for (c, v) in centroid.iter_mut().zip(&coords[i]) {
                *c += v / size as f64;
            }
        }
        let f = Family {
            id,
            seeds: members.iter().map(|i| trajs[*i].seed).collect(),
            size,
            success_rate: round(frac(&|e| e.success)),
            progress_rate: round(frac(&|e| e.made_progress)),
            mean_progress_max: round(
                members
                    .iter()
                    .map(|i| episodes[*i].progress_max)
                    .sum::<f64>()
                    / size as f64,
            ),
            stuck_episodes: count(&|e| !e.stuck.is_empty()),
            oscillating_episodes: count(&|e| !e.oscillations.is_empty()),
            unrecoverable_episodes: count(&|e| !e.unrecoverable.is_empty()),
            signature: denorm(&centroid, &rs, &labels),
        };
        if f.success_rate <= th.min_family_success {
            let reason = if f.progress_rate == 0.0 {
                "no_progress"
            } else if f.success_rate == 0.0 {
                "no_success"
            } else {
                "below_min_success"
            };
            let mut issues: Vec<(usize, String)> = vec![
                (
                    f.stuck_episodes,
                    format!("stuck in {} of {size} episodes", f.stuck_episodes),
                ),
                (
                    f.oscillating_episodes,
                    format!(
                        "oscillating in {} of {size} episodes",
                        f.oscillating_episodes
                    ),
                ),
                (
                    f.unrecoverable_episodes,
                    format!(
                        "unrecoverable metric in {} of {size} episodes",
                        f.unrecoverable_episodes
                    ),
                ),
            ];
            issues.retain(|(k, _)| *k > 0);
            issues.sort_by_key(|i| std::cmp::Reverse(i.0));
            vetoed.push(VetoedFamily {
                family: id,
                seeds: f.seeds.clone(),
                reason: reason.into(),
                success_rate: f.success_rate,
                progress_rate: f.progress_rate,
                coordinates: centroid.iter().map(|c| round(*c)).collect(),
                coordinate_fields: coordinate_fields.clone(),
                signature: f.signature.clone(),
                issues: issues.into_iter().map(|(_, s)| s).collect(),
            });
        }
        families.push(f);
    }
    let rejected = !vetoed.is_empty();
    let message = if trajs.is_empty() {
        "no trajectories".to_string()
    } else if rejected {
        format!(
            "rejected: {} of {} behavior families at or below {:.0}% success (seeds {}); revise the generated content or policy before building on it",
            vetoed.len(),
            n_fam,
            th.min_family_success * 100.0,
            vetoed
                .iter()
                .flat_map(|v| v.seeds.iter().map(u64::to_string))
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        format!(
            "accepted: all {n_fam} behavior families exceed {:.0}% success",
            th.min_family_success * 100.0
        )
    };
    Ok(Diagnostics {
        approval: (!trajs.is_empty()).then_some(!rejected),
        veto: Veto {
            rejected,
            message,
            families: vetoed,
        },
        families,
        episodes,
        ranges: rs,
    })
}

//! Timestamped measurements supplied by the recorder. Times use a run-relative scenario clock.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SAMPLES: usize = 200_000;
const MAX_WORKERS: usize = 128;

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct Telemetry {
    /// Timestamp of decoded video frame zero relative to the shared scenario marker.
    pub video_first_frame_us: u64,
    pub runtime_frames: Option<RuntimeFrames>,
    pub workers: Option<Workers>,
    pub resources: Option<Resources>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeFrames {
    pub source: String,
    pub samples: Vec<FrameSample>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FrameSample {
    /// Timestamp at the end of the measured frame interval.
    at_us: u64,
    frame_time_us: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct Workers {
    pub source: String,
    coverage_start_us: u64,
    coverage_end_us: u64,
    /// Non-overlapping busy intervals per named worker, ordered by start.
    spans: BTreeMap<String, Vec<Span>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Span {
    start_us: u64,
    end_us: u64,
}

impl Workers {
    fn busy_between(&self, start: u64, end: u64) -> Option<BTreeMap<&str, u64>> {
        if start < self.coverage_start_us || end > self.coverage_end_us {
            return None;
        }
        Some(
            self.spans
                .iter()
                .filter_map(|(name, spans)| {
                    let first = spans.partition_point(|span| span.end_us <= start);
                    let busy: u64 = spans[first..]
                        .iter()
                        .take_while(|span| span.start_us < end)
                        .map(|span| span.end_us.min(end) - span.start_us.max(start))
                        .sum();
                    (busy > 0).then_some((name.as_str(), busy))
                })
                .collect(),
        )
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct Resources {
    pub source: String,
    /// Recorder-defined scope, for example "game process tree; whole RTX 5070".
    pub scope: String,
    pub samples: Vec<ResourceSample>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceSample {
    at_us: u64,
    cpu_pct: Option<f64>,
    rss_bytes: Option<u64>,
    gpu_pct: Option<f64>,
    vram_bytes: Option<u64>,
}

fn label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 160 && !value.chars().any(char::is_control)
}

fn validate_samples<T>(samples: &[T], at: impl Fn(&T) -> u64) -> anyhow::Result<()> {
    anyhow::ensure!(
        (1..=MAX_SAMPLES).contains(&samples.len()),
        "invalid telemetry sample count"
    );
    anyhow::ensure!(
        at(&samples[samples.len() - 1]) <= super::MAX_TIME_US
            && samples.windows(2).all(|w| at(&w[0]) < at(&w[1])),
        "sample timestamps must be ordered, unique and within 24 hours"
    );
    Ok(())
}

impl Telemetry {
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        use anyhow::Context;
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let metadata = path
            .metadata()
            .with_context(|| format!("inspect telemetry {}", path.display()))?;
        anyhow::ensure!(
            metadata.is_file() && metadata.len() <= MAX_BYTES,
            "expected a regular telemetry file of at most 16 MiB"
        );
        let file =
            File::open(path).with_context(|| format!("open telemetry {}", path.display()))?;
        let mut data = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut data)?;
        anyhow::ensure!(data.len() as u64 <= MAX_BYTES, "telemetry exceeds 16 MiB");
        let result: Self = serde_json::from_slice(&data).context("parse timeline telemetry")?;
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.video_first_frame_us <= super::MAX_TIME_US,
            "video anchor exceeds 24 hours"
        );
        if let Some(track) = &self.runtime_frames {
            anyhow::ensure!(label(&track.source), "invalid runtime timing source");
            validate_samples(&track.samples, |s| s.at_us)?;
            anyhow::ensure!(
                track
                    .samples
                    .iter()
                    .all(|s| s.frame_time_us > 0 && s.frame_time_us <= s.at_us),
                "runtime frame duration must be positive and within the scenario clock"
            );
            anyhow::ensure!(
                track
                    .samples
                    .windows(2)
                    .all(|w| w[1].at_us - w[1].frame_time_us >= w[0].at_us),
                "runtime frame intervals overlap"
            );
        }
        if let Some(track) = &self.workers {
            anyhow::ensure!(
                label(&track.source)
                    && track.coverage_start_us < track.coverage_end_us
                    && track.coverage_end_us <= super::MAX_TIME_US
                    && track.spans.len() <= MAX_WORKERS,
                "invalid worker source, coverage or count"
            );
            anyhow::ensure!(
                track.spans.values().map(Vec::len).sum::<usize>() <= MAX_SAMPLES,
                "worker span count exceeds limit"
            );
            for (name, spans) in &track.spans {
                anyhow::ensure!(
                    label(name)
                        && spans.iter().all(|s| s.start_us < s.end_us
                            && s.start_us >= track.coverage_start_us
                            && s.end_us <= track.coverage_end_us)
                        && spans.windows(2).all(|w| w[0].end_us <= w[1].start_us),
                    "invalid, unordered or overlapping worker spans for {name}"
                );
            }
        }
        if let Some(track) = &self.resources {
            anyhow::ensure!(
                label(&track.source) && label(&track.scope),
                "invalid resource source or scope"
            );
            validate_samples(&track.samples, |s| s.at_us)?;
            anyhow::ensure!(
                track
                    .samples
                    .iter()
                    .all(|s| s.cpu_pct.is_none_or(|v| v.is_finite() && v >= 0.0)
                        && s.gpu_pct.is_none_or(|v| (0.0..=100.0).contains(&v))
                        && (s.cpu_pct.is_some()
                            || s.gpu_pct.is_some()
                            || s.rss_bytes.is_some()
                            || s.vram_bytes.is_some())),
                "invalid or empty resource sample"
            );
        }
        Ok(())
    }

    pub fn timing_summary(&self, budget: u64) -> Option<Value> {
        self.runtime_frames.as_ref().map(|track| json!({
            "source": track.source,
            "stats": super::interval_stats(track.samples.iter().map(|s| s.frame_time_us), budget),
        }))
    }

    pub fn at(
        &self,
        previous: Option<u64>,
        at: u64,
        timing_gap: u64,
        resource_age: u64,
        budget: u64,
    ) -> Value {
        let frame = self.runtime_frames.as_ref().and_then(|t| {
            let right = t.samples.partition_point(|s| s.at_us < at);
            right
                .checked_sub(1)
                .and_then(|i| t.samples.get(i))
                .into_iter()
                .chain(t.samples.get(right))
                .min_by_key(|s| s.at_us.abs_diff(at))
                .filter(|s| s.at_us.abs_diff(at) <= timing_gap)
                .map(|s| {
                    json!({"sample":s,"distance_us":s.at_us.abs_diff(at),
                    "interval_fps":1_000_000.0 / s.frame_time_us as f64,
                    "worker_busy_us":self.workers.as_ref().and_then(|w| w.busy_between(s.at_us-s.frame_time_us,s.at_us)),
                    "over_budget":s.frame_time_us > budget,"stutter":s.frame_time_us > budget * 2})
                })
        });
        let resources = self.resources.as_ref().and_then(|t| {
            let i = t
                .samples
                .partition_point(|s| s.at_us <= at)
                .checked_sub(1)?;
            let sample = &t.samples[i];
            let age = at - sample.at_us;
            (age <= resource_age).then(|| json!({"sample":sample,"age_us":age}))
        });
        let workers = self
            .workers
            .as_ref()
            .zip(previous)
            .and_then(|(t, start)| t.busy_between(start, at));
        json!({"runtime_frame":frame, "worker_busy_us":workers, "resources":resources})
    }
}

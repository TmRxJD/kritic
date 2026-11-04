//! Deterministic comparison of video frames and measurements on a shared scenario timeline.
mod telemetry;
mod video;

use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{
    io::{self, BufWriter, Write},
    path::PathBuf,
    time::Duration,
};
use telemetry::Telemetry;
use video::Video;

const MAX_TIME_US: u64 = 24 * 60 * 60 * 1_000_000;
const MAX_REPORT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Args, Debug)]
pub struct TimelineArgs {
    #[command(subcommand)]
    command: TimelineCommand,
}

#[derive(Subcommand, Debug)]
enum TimelineCommand {
    /// Compare decoded frames and join optional synchronized telemetry; outputs NDJSON.
    Compare(CompareArgs),
    /// Print the JSON Schema for the optional per-run telemetry file.
    Schema,
}

#[derive(Args, Debug)]
struct CompareArgs {
    #[arg(long)]
    baseline: PathBuf,
    #[arg(long)]
    candidate: PathBuf,
    #[arg(long)]
    baseline_telemetry: Option<PathBuf>,
    #[arg(long)]
    candidate_telemetry: Option<PathBuf>,
    /// Maximum baseline/candidate presentation timestamp difference.
    #[arg(long, default_value_t = 8_000, value_parser = clap::value_parser!(u64).range(0..=250_000))]
    match_us: u64,
    /// Maximum difference from a measured runtime frame timestamp.
    #[arg(long, default_value_t = 8_000, value_parser = clap::value_parser!(u64).range(0..=250_000))]
    timing_gap_us: u64,
    /// Maximum age of the preceding resource sample.
    #[arg(long, default_value_t = 250_000, value_parser = clap::value_parser!(u64).range(0..=5_000_000))]
    resource_age_us: u64,
    /// Target duration for a frame. Stutter is more than twice this duration.
    #[arg(long, default_value_t = 16_667, value_parser = clap::value_parser!(u64).range(1..=1_000_000))]
    frame_budget_us: u64,
    /// A pixel counts as changed if any decoded RGB channel differs by at least this much.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u8).range(1..=255))]
    pixel_threshold: u8,
    /// Total time limit for probing, decoding and comparing the two inputs.
    #[arg(long, default_value_t = 180, value_parser = clap::value_parser!(u64).range(1..=1800))]
    timeout_seconds: u64,
}

pub async fn run(args: TimelineArgs) -> Result<(), String> {
    let result = match args.command {
        TimelineCommand::Schema => {
            serde_json::to_writer_pretty(io::stdout().lock(), &schemars::schema_for!(Telemetry))
                .map_err(anyhow::Error::from)
        }
        TimelineCommand::Compare(args) => {
            tokio::time::timeout(Duration::from_secs(args.timeout_seconds), compare(&args))
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("timeline comparison timed out")))
        }
    };
    result.map_err(|e| format!("{e:#}"))
}

fn interval_stats(intervals: impl Iterator<Item = u64>, budget: u64) -> Value {
    let mut intervals: Vec<_> = intervals.collect();
    if intervals.is_empty() {
        return Value::Null;
    }
    intervals.sort_unstable();
    let count = intervals.len();
    let total: u64 = intervals.iter().sum();
    let percentile = |p: usize| intervals[(p * count).div_ceil(100) - 1];
    let over_budget = count - intervals.partition_point(|&value| value <= budget);
    let stutters = count - intervals.partition_point(|&value| value <= budget * 2);
    json!({
        "intervals": count, "avg_fps": count as f64 * 1_000_000.0 / total as f64,
        "p50_us": percentile(50), "p95_us": percentile(95), "p99_us": percentile(99),
        "max_us": intervals[count - 1], "over_budget": over_budget, "stutters": stutters,
    })
}

fn pixel_diff(left: &[u8], right: &[u8], threshold: u8) -> Value {
    if left == right {
        return json!({"exact_match":true,"mean_channel_delta":0.0,"changed_pixel_fraction":0.0});
    }
    let mut changed = 0;
    let mut total = 0_u64;
    let (left_pixels, _) = left.as_chunks::<3>();
    let (right_pixels, _) = right.as_chunks::<3>();
    for (a, b) in left_pixels.iter().zip(right_pixels) {
        let deltas = [
            a[0].abs_diff(b[0]),
            a[1].abs_diff(b[1]),
            a[2].abs_diff(b[2]),
        ];
        changed += usize::from(deltas.iter().any(|&v| v >= threshold));
        total += deltas.iter().map(|&v| u64::from(v)).sum::<u64>();
    }
    json!({
        "exact_match": false,
        "mean_channel_delta": total as f64 / left.len() as f64,
        "changed_pixel_fraction": changed as f64 / (left.len() / 3) as f64,
    })
}

struct Run {
    video: Video,
    telemetry: Telemetry,
    pixels: Vec<u8>,
    previous_pixels: Vec<u8>,
    index: usize,
}

impl Run {
    async fn open(
        path: &std::path::Path,
        telemetry: Option<&std::path::Path>,
    ) -> anyhow::Result<Self> {
        let telemetry = Telemetry::load(telemetry)?;
        let video = Video::open(path, telemetry.video_first_frame_us).await?;
        let pixels = video.buffer();
        let previous_pixels = video.buffer();
        Ok(Self {
            video,
            telemetry,
            pixels,
            previous_pixels,
            index: 0,
        })
    }

    fn next_time(&self) -> Option<u64> {
        self.video.times.get(self.index).copied()
    }

    fn describe(&self, budget: u64) -> Value {
        json!({
            "frame_count": self.video.times.len(), "width":self.video.width,"height":self.video.height,
            "video_first_frame_us": self.telemetry.video_first_frame_us,
            "video_cadence": interval_stats(self.video.times.windows(2).map(|w| w[1] - w[0]), budget),
            "runtime_timing": self.telemetry.timing_summary(budget),
            "worker_source": self.telemetry.workers.as_ref().map(|w| &w.source),
            "resource_source": self.telemetry.resources.as_ref().map(|r| &r.source),
            "resource_scope": self.telemetry.resources.as_ref().map(|r| &r.scope),
        })
    }

    async fn take(&mut self, at: u64, args: &CompareArgs) -> anyhow::Result<Value> {
        std::mem::swap(&mut self.pixels, &mut self.previous_pixels);
        self.video.read(&mut self.pixels).await?;
        let index = self.index;
        let previous = index.checked_sub(1).map(|i| self.video.times[i]);
        self.index += 1;
        Ok(json!({
            "index":index,"at_us":at,"video_interval_us":previous.map(|p| at-p),
            "video_interval_fps":previous.map(|p| 1_000_000.0 / (at-p) as f64),
            "previous_frame_visual":previous.map(|_| pixel_diff(&self.previous_pixels,&self.pixels,args.pixel_threshold)),
            "telemetry":self.telemetry.at(previous,at,args.timing_gap_us,args.resource_age_us,args.frame_budget_us),
        }))
    }
}

fn record(out: &mut impl Write, remaining: &mut usize, value: Value) -> anyhow::Result<()> {
    let line = serde_json::to_vec(&value)?;
    let size = line.len() + 1;
    anyhow::ensure!(size <= *remaining, "timeline report exceeds 128 MiB");
    *remaining -= size;
    out.write_all(&line)?;
    out.write_all(b"\n")?;
    Ok(())
}

async fn compare(args: &CompareArgs) -> anyhow::Result<()> {
    let mut left = Run::open(&args.baseline, args.baseline_telemetry.as_deref()).await?;
    let mut right = Run::open(&args.candidate, args.candidate_telemetry.as_deref()).await?;
    let same_raster =
        (left.video.width, left.video.height) == (right.video.width, right.video.height);
    let mut out = BufWriter::new(io::stdout().lock());
    let mut remaining = MAX_REPORT_BYTES;
    record(
        &mut out,
        &mut remaining,
        json!({
            "type":"header","schema_version":1,"match_us":args.match_us,"timing_gap_us":args.timing_gap_us,
            "resource_age_us":args.resource_age_us,"frame_budget_us":args.frame_budget_us,
            "pixel_threshold":args.pixel_threshold,"same_raster":same_raster,
            "baseline":left.describe(args.frame_budget_us),"candidate":right.describe(args.frame_budget_us),
        }),
    )?;
    let (mut matched, mut left_only, mut right_only) = (0, 0, 0);
    loop {
        let (a, b) = match (left.next_time(), right.next_time()) {
            (Some(a), Some(b)) if a.abs_diff(b) <= args.match_us => (Some(a), Some(b)),
            (Some(a), Some(b)) if a < b => (Some(a), None),
            (Some(_), Some(b)) => (None, Some(b)),
            (a, b) => (a, b),
        };
        let (baseline, candidate, visual) = match (a, b) {
            (Some(a), Some(b)) => {
                let baseline = left.take(a, args).await?;
                let candidate = right.take(b, args).await?;
                matched += 1;
                let visual = same_raster
                    .then(|| pixel_diff(&left.pixels, &right.pixels, args.pixel_threshold));
                (Some(baseline), Some(candidate), visual)
            }
            (Some(a), None) => {
                left_only += 1;
                (Some(left.take(a, args).await?), None, None)
            }
            (None, Some(b)) => {
                right_only += 1;
                (None, Some(right.take(b, args).await?), None)
            }
            (None, None) => break,
        };
        record(
            &mut out,
            &mut remaining,
            json!({
                "type":"frame","baseline":baseline,"candidate":candidate,"visual":visual,
                "candidate_minus_baseline_us":a.zip(b).map(|(a,b)| b as i64 - a as i64),
            }),
        )?;
    }
    left.video.finish().await?;
    right.video.finish().await?;
    let visual_status = match (matched, same_raster) {
        (0, _) => "no_matches",
        (_, false) => "different_rasters",
        (_, true) => "compared",
    };
    record(
        &mut out,
        &mut remaining,
        json!({
            "type":"complete","matched_frames":matched,"baseline_only_frames":left_only,
            "candidate_only_frames":right_only,
            "visual_status":visual_status,"visual_comparisons":if same_raster { matched } else { 0 },
        }),
    )?;
    out.flush()?;
    Ok(())
}

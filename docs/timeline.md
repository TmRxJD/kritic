# Video and performance timeline

`kritic timeline compare` decodes baseline and candidate videos and emits one NDJSON
row for each matched pair or unmatched frame. It can join frame timing, worker activity
and resource measurements on the same scenario clock. The comparison is deterministic
and works with vision disabled. Install `ffmpeg` and `ffprobe` from the same distribution
and put both on PATH. Supported local containers include MP4, MOV, MKV, WebM and AVI.

```sh
kritic timeline compare --baseline before.mkv --candidate after.mkv \
  --baseline-telemetry before.json --candidate-telemetry after.json \
  --frame-budget-us 16667 > timeline.ndjson
```

For video-only comparison, omit the telemetry flags. Both videos then start at zero.
Record the same scripted workload, capture geometry and settings on each run. Keep
the source clips available for inspection using the frame index or presentation time
in the report.

## Synchronize the captures

Use a repeatable scenario marker and express each recorder's measurements as elapsed
microseconds from that marker. `video_first_frame_us` is the timestamp of the first
decoded frame on that clock. Each later frame uses the video's presentation timestamp
delta from frame zero. An anchor can be supplied without any metric tracks. Convert
independent recorder clocks using observed synchronization markers. If the offset or
drift is unknown, leave those tracks out of the comparison.

Run `kritic timeline schema` for the exact sidecar schema. Each run takes its own
sidecar. The recorder supplies these samples and their provenance.

```json
{
  "video_first_frame_us": 0,
  "runtime_frames": {
    "source": "application presentation intervals",
    "samples": [
      {"at_us": 16667, "frame_time_us": 16667},
      {"at_us": 56667, "frame_time_us": 40000}
    ]
  },
  "workers": {
    "source": "instrumented worker task spans",
    "coverage_start_us": 0,
    "coverage_end_us": 56667,
    "spans": {
      "geometry": [{"start_us": 22000, "end_us": 43000}]
    }
  },
  "resources": {
    "source": "synchronized OS sampler",
    "scope": "application process tree CPU and RSS; whole GPU utilization and VRAM",
    "samples": [
      {"at_us": 40000, "cpu_pct": 83.5, "rss_bytes": 2147483648, "gpu_pct": 71, "vram_bytes": 3221225472}
    ]
  }
}
```

- Runtime frame samples end at `at_us`. Supply ordered, non-overlapping intervals
  from one timing source. A presentation trace, engine frame intervals, and browser
  animation callback intervals measure different things. Label the source and compare
  like with like. FPS is count divided by the sum of measured interval durations.
- Worker spans represent measured busy time for each worker. Sort them and merge
  overlaps per worker before export. For browser workers, instrument actual task start
  and end or export identified worker-thread spans from a browser trace. Aggregate
  process thread counts cannot establish individual worker activity.
- Resource samples use the recorder's stated scope. Process-tree CPU percent may
  exceed 100 across cores. GPU percent ranges from 0 to 100. Optional fields may be
  omitted and missing measurements remain null.
- This command consumes recorder exports. The existing aggregate resource report
  lacks per-sample timestamps, so it cannot be joined as-is.

## Read the timeline

The header reports separate video cadence and runtime timing summaries, including
average FPS, p50/p95/p99, maximum interval, budget misses and stutters. A stutter is
an interval longer than twice the configured frame budget. Runtime timing statistics
cover supplied intervals and exclude unmeasured gaps.

The matcher walks both streams in presentation order and pairs the next frames if
their timestamps differ by at most `--match-us` (8,000 µs by default). Otherwise it
emits the earlier frame unmatched. Every decoded frame appears once. Choose a
tolerance smaller than the frame interval when you need strict temporal matching.

Each row reports the actual timestamp difference, decoded RGB pixel difference for
matching raster dimensions, and each run's change from its own previous frame.
`exact_match` detects equal decoded rasters. The changed-pixel fraction counts pixels
with any channel delta at least `--pixel-threshold`. Compression, color conversion
and capture differences can produce pixel differences. Raster comparisons ignore
display rotation metadata and do not resize either video.

Runtime measurements join by nearest timestamp within `--timing-gap-us`. Worker
busy time integrates spans across the interval between consecutive video frames.
The matched runtime sample also reports worker busy time over its own measured
interval, so an earlier task inside a long runtime frame remains visible.
Resource metrics use the preceding sample within `--resource-age-us`, and report its
age. A null value indicates missing or out-of-window evidence. An empty worker map
means the recorder covered the interval and reported no busy workers.

Inspect a hitch by finding a frame row with `runtime_frame.stutter`, then reviewing
its visual difference, temporal change, worker busy times and resource sample age.
The timeline shows co-occurrence. Use repeated captures or a controlled experiment
to establish causes. For semantic visual review, pass selected still frames to
`visual_critic`; the timeline itself uses pixel measurements.

Use a report only when the command exits successfully and the last record is
`"type":"complete"`. Check `visual_status` and `visual_comparisons` in that record
before drawing visual conclusions. Partial output can remain after a decoder failure. Each video
is limited to 2 GiB, 18,000 frames, and 8,294,400 pixels per frame. Sidecars are
limited to 16 MiB each, output to 128 MiB, and the default run timeout is 180 seconds.

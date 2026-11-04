//! Decode the same non-attached video stream whose presentation timestamps were probed.
use serde::Deserialize;
use std::{io, path::Path, process::Stdio};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{Child, ChildStdout, Command},
};

const CONTAINERS: &str = "mov,matroska,webm,avi";
const MAX_PROBE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_VIDEO_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_PIXELS: usize = 3840 * 2160;
pub(super) const MAX_FRAMES: usize = 18_000;

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
    frames: Vec<Frame>,
}

#[derive(Deserialize)]
struct Stream {
    width: usize,
    height: usize,
    time_base: String,
}

#[derive(Deserialize)]
struct Frame {
    pts: i64,
    width: usize,
    height: usize,
}

pub(super) struct Video {
    pub width: usize,
    pub height: usize,
    pub times: Vec<u64>,
    frame_bytes: usize,
    decoder: Child,
    pixels: BufReader<ChildStdout>,
}

fn command(exe: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    cmd
}

fn restrict_input(cmd: &mut Command) {
    // Disallow remote inputs and playlist demuxers. MOV external references remain off.
    cmd.args([
        "-protocol_whitelist",
        "file",
        "-format_whitelist",
        CONTAINERS,
    ]);
    // Allow decoder padding while bounding allocations before raster validation.
    cmd.args(["-max_pixels", "16777216", "-threads", "2"]);
}

fn timestamps(frames: &[Frame], time_base: &str, start: u64) -> anyhow::Result<Vec<u64>> {
    let (num, den) = time_base
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("missing video time base"))?;
    let (num, den) = (num.parse::<i32>()?, den.parse::<i32>()?);
    anyhow::ensure!(num > 0 && den > 0, "invalid video time base");
    let first = i128::from(frames[0].pts);
    let times = frames
        .iter()
        .map(|frame| {
            let ticks = i128::from(frame.pts) - first;
            let micros = (ticks * i128::from(num) * 1_000_000) / i128::from(den);
            let at = u64::try_from(micros)?
                .checked_add(start)
                .ok_or_else(|| anyhow::anyhow!("video timestamp overflow"))?;
            anyhow::ensure!(
                at <= super::MAX_TIME_US,
                "video exceeds the 24-hour timeline bound"
            );
            Ok(at)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    anyhow::ensure!(
        times.windows(2).all(|w| w[0] < w[1]),
        "video presentation timestamps must be strictly increasing at microsecond precision"
    );
    Ok(times)
}

impl Video {
    pub async fn open(input: &Path, start: u64) -> anyhow::Result<Self> {
        use anyhow::Context;
        let input = input
            .canonicalize()
            .with_context(|| format!("resolve video {}", input.display()))?;
        let metadata = input.metadata()?;
        anyhow::ensure!(
            metadata.is_file() && metadata.len() <= MAX_VIDEO_BYTES,
            "expected a regular local video file of at most 2 GiB"
        );
        let mut cmd = command("ffprobe");
        restrict_input(&mut cmd);
        let mut probe = cmd
            .args([
                "-v",
                "error",
                "-select_streams",
                "V:0",
                "-show_frames",
                "-show_streams",
                "-show_entries",
                "stream=width,height,time_base:frame=pts,width,height",
                "-of",
                "json",
            ])
            .arg(&input)
            .spawn()
            .context("start ffprobe (install FFmpeg and add it to PATH)")?;
        let mut output = Vec::new();
        probe
            .stdout
            .take()
            .context("ffprobe stdout unavailable")?
            .take(MAX_PROBE_BYTES + 1)
            .read_to_end(&mut output)
            .await?;
        anyhow::ensure!(
            output.len() as u64 <= MAX_PROBE_BYTES,
            "ffprobe report exceeds 8 MiB"
        );
        anyhow::ensure!(
            probe.wait().await?.success(),
            "ffprobe failed for {}",
            input.display()
        );
        let data: Probe = serde_json::from_slice(&output)
            .context("read video frames and timestamps from ffprobe")?;
        let [stream] = data.streams.as_slice() else {
            anyhow::bail!("expected one selected video stream")
        };
        anyhow::ensure!(
            (2..=MAX_FRAMES).contains(&data.frames.len()),
            "expected 2–{MAX_FRAMES} decoded frames; trim long captures"
        );
        let frame_bytes = stream
            .width
            .checked_mul(stream.height)
            .filter(|&n| (1..=MAX_PIXELS).contains(&n))
            .context("video exceeds the 8,294,400 pixel frame limit")?
            * 3;
        anyhow::ensure!(
            data.frames
                .iter()
                .all(|f| f.width == stream.width && f.height == stream.height),
            "video changes raster dimensions mid-stream"
        );
        let times = timestamps(&data.frames, &stream.time_base, start)?;

        let mut cmd = command("ffmpeg");
        restrict_input(&mut cmd);
        let mut decoder = cmd
            .args(["-nostdin", "-v", "error", "-xerror", "-noautorotate", "-i"])
            .arg(&input)
            .args([
                "-map",
                "0:V:0",
                "-an",
                "-sn",
                "-dn",
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "rgb24",
                "-threads:v",
                "1",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .spawn()
            .context("start ffmpeg (install FFmpeg and add it to PATH)")?;
        let pixels = BufReader::new(decoder.stdout.take().context("ffmpeg stdout unavailable")?);
        Ok(Self {
            width: stream.width,
            height: stream.height,
            times,
            frame_bytes,
            decoder,
            pixels,
        })
    }

    pub fn buffer(&self) -> Vec<u8> {
        vec![0; self.frame_bytes]
    }

    pub async fn read(&mut self, buffer: &mut [u8]) -> anyhow::Result<()> {
        use anyhow::Context;
        self.pixels
            .read_exact(buffer)
            .await
            .context("ffmpeg ended before the probed frame count")?;
        Ok(())
    }

    pub async fn finish(mut self) -> anyhow::Result<()> {
        match self.pixels.read_u8().await {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {}
            Err(e) => return Err(e.into()),
            Ok(_) => anyhow::bail!("ffmpeg produced more pixels than the probed frame count"),
        }
        anyhow::ensure!(self.decoder.wait().await?.success(), "ffmpeg failed");
        Ok(())
    }
}

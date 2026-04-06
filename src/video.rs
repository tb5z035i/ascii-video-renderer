use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct VideoMetadata {
    pub width: usize,
    pub height: usize,
    pub fps: f64,
}

impl VideoMetadata {
    pub fn frame_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64((1.0 / self.fps).max(1.0 / 240.0))
    }
}

#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    pub decode_instant: Instant,
    pub sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameToken(pub u64);

#[derive(Clone)]
pub struct LatestFrameSlot {
    inner: Arc<LatestFrameInner>,
}

struct LatestFrameInner {
    frame: Mutex<Option<Arc<DecodedFrame>>>,
    sequence: AtomicU64,
}

impl LatestFrameSlot {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(LatestFrameInner {
                frame: Mutex::new(None),
                sequence: AtomicU64::new(0),
            }),
        }
    }

    pub fn publish(&self, frame: DecodedFrame) {
        let sequence = self.inner.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let mut frame = frame;
        frame.sequence = sequence;

        let mut guard = self
            .inner
            .frame
            .lock()
            .expect("latest frame mutex poisoned");
        *guard = Some(Arc::new(frame));
    }

    pub fn latest(&self) -> Option<Arc<DecodedFrame>> {
        self.inner
            .frame
            .lock()
            .expect("latest frame mutex poisoned")
            .clone()
    }

    pub fn latest_if_newer(&self, token: FrameToken) -> Option<Arc<DecodedFrame>> {
        let frame = self.latest()?;
        (frame.sequence > token.0).then_some(frame)
    }
}

pub struct VideoDecoder {
    metadata: VideoMetadata,
    latest_frame: LatestFrameSlot,
    worker: Option<JoinHandle<Result<()>>>,
    ffmpeg_child: Option<Child>,
    stop_requested: Arc<AtomicBool>,
}

impl VideoDecoder {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let metadata = probe_video(&path)?;

        let latest_frame = LatestFrameSlot::new();
        let stop_requested = Arc::new(AtomicBool::new(false));
        let mut ffmpeg_child = spawn_ffmpeg_decoder(&path)?;
        let stdout = ffmpeg_child
            .stdout
            .take()
            .context("ffmpeg stdout was not piped")?;

        let worker_metadata = metadata.clone();
        let worker_slot = latest_frame.clone();
        let worker_stop = Arc::clone(&stop_requested);

        let worker = thread::Builder::new()
            .name("ffmpeg-decoder".into())
            .spawn(move || decode_frames(stdout, worker_metadata, worker_slot, worker_stop))
            .context("failed to spawn decoder thread")?;

        Ok(Self {
            metadata,
            latest_frame,
            worker: Some(worker),
            ffmpeg_child: Some(ffmpeg_child),
            stop_requested,
        })
    }

    pub fn metadata(&self) -> &VideoMetadata {
        &self.metadata
    }

    pub fn latest_frame(&self) -> Option<Arc<DecodedFrame>> {
        self.latest_frame.latest()
    }

    pub fn latest_frame_if_newer(&self, token: FrameToken) -> Option<Arc<DecodedFrame>> {
        self.latest_frame.latest_if_newer(token)
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Relaxed);

        if let Some(child) = self.ffmpeg_child.as_mut() {
            let _ = child.kill();
        }

        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }

        if let Some(mut child) = self.ffmpeg_child.take() {
            let _ = child.wait();
        }
    }
}

fn decode_frames(
    stdout: ChildStdout,
    metadata: VideoMetadata,
    latest_frame: LatestFrameSlot,
    stop_requested: Arc<AtomicBool>,
) -> Result<()> {
    let frame_len = metadata.width * metadata.height;
    let mut reader = BufReader::new(stdout);

    loop {
        if stop_requested.load(Ordering::Relaxed) {
            break;
        }

        let mut pixels = vec![0_u8; frame_len];
        match reader.read_exact(&mut pixels) {
            Ok(()) => {
                latest_frame.publish(DecodedFrame {
                    width: metadata.width,
                    height: metadata.height,
                    pixels,
                    decode_instant: Instant::now(),
                    sequence: 0,
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(err).context("failed to read ffmpeg frame from stdout"),
        }
    }

    Ok(())
}

fn spawn_ffmpeg_decoder(path: &Path) -> Result<Child> {
    Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-nostdin",
            "-i",
            path.to_str().context("input path is not valid UTF-8")?,
            "-an",
            "-sn",
            "-pix_fmt",
            "gray",
            "-f",
            "rawvideo",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to launch ffmpeg for {}", path.display()))
}

pub fn probe_video(path: &Path) -> Result<VideoMetadata> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,avg_frame_rate,r_frame_rate",
            "-of",
            "json",
            path.to_str().context("input path is not valid UTF-8")?,
        ])
        .output()
        .with_context(|| format!("failed to run ffprobe for {}", path.display()))?;

    if !output.status.success() {
        bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let parsed: ProbeOutput =
        serde_json::from_slice(&output.stdout).context("invalid ffprobe JSON")?;
    let stream = parsed
        .streams
        .into_iter()
        .next()
        .context("ffprobe did not return a video stream")?;

    let fps = parse_fps(stream.avg_frame_rate.as_deref())
        .or_else(|| parse_fps(stream.r_frame_rate.as_deref()))
        .unwrap_or(60.0);

    Ok(VideoMetadata {
        width: stream.width.context("ffprobe missing width")?,
        height: stream.height.context("ffprobe missing height")?,
        fps,
    })
}

pub fn parse_fps(value: Option<&str>) -> Option<f64> {
    let raw = value?;
    if raw.trim().is_empty() || raw == "0/0" || raw == "N/A" {
        return None;
    }

    let (numerator, denominator) = raw.split_once('/')?;
    let numerator: f64 = numerator.parse().ok()?;
    let denominator: f64 = denominator.parse().ok()?;

    if denominator == 0.0 {
        None
    } else {
        Some(numerator / denominator)
    }
}

#[derive(Debug, Deserialize)]
struct ProbeOutput {
    streams: Vec<ProbeStream>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    width: Option<usize>,
    height: Option<usize>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::parse_fps;

    #[test]
    fn parses_fractional_fps() {
        let fps = parse_fps(Some("60000/1001")).unwrap();
        assert!((fps - 59.94005994).abs() < 0.0001);
    }

    #[test]
    fn rejects_invalid_fps_inputs() {
        assert_eq!(parse_fps(Some("0/0")), None);
        assert_eq!(parse_fps(Some("abc")), None);
        assert_eq!(parse_fps(None), None);
    }
}

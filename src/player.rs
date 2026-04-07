use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::ascii::{AsciiRenderer, FpsAverager};
use crate::terminal::{compute_render_layout, PlaybackLayout, TerminalSession};
use crate::video::{FrameToken, VideoDecoder};

#[derive(Debug, Clone)]
pub struct PlayerOptions {
    pub input: PathBuf,
    pub max_frames: Option<u64>,
}

pub struct Player {
    options: PlayerOptions,
    decoder: VideoDecoder,
    terminal: TerminalSession,
    renderer: AsciiRenderer,
    latest_token: FrameToken,
    current_layout: Option<PlaybackLayout>,
    stats: PlaybackStats,
}

impl Player {
    pub fn new(options: PlayerOptions) -> Result<Self> {
        let terminal = TerminalSession::enter()?;
        let decoder = VideoDecoder::open(&options.input)?;
        let renderer = AsciiRenderer::new()?;

        Ok(Self {
            options,
            decoder,
            terminal,
            renderer,
            latest_token: FrameToken(0),
            stats: PlaybackStats::default(),
            current_layout: None,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        let frame_interval = self.decoder.metadata().frame_duration();
        let mut current_frame = None;

        loop {
            if self.terminal.poll_exit_request(Duration::ZERO)? {
                break;
            }

            let loop_started = Instant::now();
            let mut reused_cached_frame = false;
            if let Some(frame) = self.decoder.latest_frame_if_newer(self.latest_token) {
                self.latest_token = FrameToken(frame.sequence);
                current_frame = Some(frame);
            } else if current_frame.is_none() {
                current_frame = self.decoder.latest_frame();
            } else {
                reused_cached_frame = true;
            }

            let Some(frame) = current_frame.clone() else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };

            if self.decoder.is_finished() && reused_cached_frame {
                break;
            }

            let terminal_size = self.terminal.current_size()?;
            let layout = compute_render_layout(
                terminal_size,
                self.decoder.metadata().width as u32,
                self.decoder.metadata().height as u32,
            );

            let needs_rebuild = self
                .current_layout
                .as_ref()
                .map(|previous| {
                    (previous.cell_aspect_ratio - layout.cell_aspect_ratio).abs() > 0.01
                })
                .unwrap_or(true);

            if needs_rebuild {
                self.renderer.rebuild_glyph_bank(layout.cell_aspect_ratio)?;
            }

            let rendered = self.renderer.render_frame(&frame, &layout);

            let render_time = Instant::now();
            self.stats.observe(
                render_time,
                frame.decode_instant,
                layout.render_cols as usize,
                layout.render_rows as usize,
            );
            let status = self.stats.status_line(usize::from(layout.terminal_cols));
            self.terminal
                .render_frame(layout, &rendered.rows, &status)?;
            self.current_layout = Some(layout);

            if let Some(max_frames) = self.options.max_frames {
                if self.stats.rendered_frames >= max_frames {
                    break;
                }
            }

            let elapsed = loop_started.elapsed();
            if elapsed < frame_interval {
                let sleep_for = frame_interval - elapsed;
                if self.terminal.poll_exit_request(sleep_for)? {
                    break;
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
struct PlaybackStats {
    rendered_frames: u64,
    fps: FpsAverager,
    recent_latency_ms: f64,
    render_cols: usize,
    render_rows: usize,
}

impl Default for PlaybackStats {
    fn default() -> Self {
        Self {
            rendered_frames: 0,
            fps: FpsAverager::new(120),
            recent_latency_ms: 0.0,
            render_cols: 0,
            render_rows: 0,
        }
    }
}

impl PlaybackStats {
    fn observe(
        &mut self,
        rendered_at: Instant,
        decode_completed_at: Instant,
        cols: usize,
        rows: usize,
    ) {
        self.rendered_frames += 1;
        self.render_cols = cols;
        self.render_rows = rows;
        self.fps.push(rendered_at);

        let latency = rendered_at.saturating_duration_since(decode_completed_at);
        self.recent_latency_ms = latency.as_secs_f64() * 1_000.0;
    }

    fn status_line(&self, width: usize) -> String {
        let mut text = format!(
            " fps {:>5.1} | latency {:>6.1} ms | grid {:>3}x{:<3} ",
            self.fps.fps(),
            self.recent_latency_ms,
            self.render_cols,
            self.render_rows
        );

        if text.len() < width {
            text.push_str(&" ".repeat(width - text.len()));
        } else {
            text.truncate(width);
        }

        text
    }
}

#[cfg(test)]
mod tests {
    use super::PlaybackStats;
    use std::time::{Duration, Instant};

    #[test]
    fn status_line_pads_to_width() {
        let mut stats = PlaybackStats::default();
        let now = Instant::now();
        stats.observe(now + Duration::from_millis(16), now, 120, 40);
        let line = stats.status_line(80);
        assert_eq!(line.len(), 80);
        assert!(line.contains("latency"));
    }
}

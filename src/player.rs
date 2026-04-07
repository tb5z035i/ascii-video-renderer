use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::ascii::{AsciiGrid, FpsAverager};
use crate::engine::{AsciiEngine, RenderAlgorithm};
use crate::terminal::{compute_render_layout, PlaybackLayout, TerminalEvent, TerminalSession};
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
    renderer: AsciiEngine,
    latest_token: FrameToken,
    current_layout: Option<PlaybackLayout>,
    stats: PlaybackStats,
}

impl Player {
    pub fn new(options: PlayerOptions) -> Result<Self> {
        let terminal = TerminalSession::enter()?;
        let decoder = VideoDecoder::open(&options.input)?;
        let renderer = AsciiEngine::new(
            RenderAlgorithm::Classic,
            terminal.current_size()?.cell_aspect_ratio,
        )?;

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
            if self.handle_terminal_event(Duration::ZERO)? {
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
            self.renderer
                .prepare_for_cell_aspect(layout.cell_aspect_ratio)?;
            let rendered = self.renderer.render_grayscale_ansi(
                &frame.pixels,
                frame.width,
                frame.height,
                AsciiGrid {
                    columns: layout.render_cols as usize,
                    rows: layout.render_rows as usize,
                },
            )?;

            let render_time = Instant::now();
            self.stats.observe(
                render_time,
                frame.decode_instant,
                layout.render_cols as usize,
                layout.render_rows as usize,
                self.renderer.algorithm(),
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
                if self.handle_terminal_event(sleep_for)? {
                    break;
                }
            }
        }

        Ok(())
    }

    fn handle_terminal_event(&mut self, timeout: Duration) -> Result<bool> {
        match self.terminal.poll_event(timeout)? {
            TerminalEvent::None => Ok(false),
            TerminalEvent::Exit => Ok(true),
            TerminalEvent::ToggleRenderer => {
                let next_algorithm = self.renderer.algorithm().next();
                let cell_aspect = self
                    .current_layout
                    .map(|layout| layout.cell_aspect_ratio)
                    .unwrap_or_else(|| {
                        self.terminal
                            .current_size()
                            .map(|size| size.cell_aspect_ratio)
                            .unwrap_or(2.0)
                    });
                self.renderer.set_algorithm(next_algorithm, cell_aspect)?;
                self.current_layout = None;
                Ok(false)
            }
        }
    }
}

#[derive(Debug)]
struct PlaybackStats {
    rendered_frames: u64,
    fps: FpsAverager,
    recent_latency_ms: f64,
    render_cols: usize,
    render_rows: usize,
    renderer_label: &'static str,
}

impl Default for PlaybackStats {
    fn default() -> Self {
        Self {
            rendered_frames: 0,
            fps: FpsAverager::new(120),
            recent_latency_ms: 0.0,
            render_cols: 0,
            render_rows: 0,
            renderer_label: RenderAlgorithm::Classic.label(),
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
        algorithm: RenderAlgorithm,
    ) {
        self.rendered_frames += 1;
        self.render_cols = cols;
        self.render_rows = rows;
        self.renderer_label = algorithm.label();
        self.fps.push(rendered_at);

        let latency = rendered_at.saturating_duration_since(decode_completed_at);
        self.recent_latency_ms = latency.as_secs_f64() * 1_000.0;
    }

    fn status_line(&self, width: usize) -> String {
        let mut text = format!(
            " fps {:>5.1} | latency {:>6.1} ms | grid {:>3}x{:<3} | mode {:<7} ",
            self.fps.fps(),
            self.recent_latency_ms,
            self.render_cols,
            self.render_rows,
            self.renderer_label,
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
    use crate::engine::RenderAlgorithm;
    use std::time::{Duration, Instant};

    #[test]
    fn status_line_pads_to_width() {
        let mut stats = PlaybackStats::default();
        let now = Instant::now();
        stats.observe(
            now + Duration::from_millis(16),
            now,
            120,
            40,
            RenderAlgorithm::Classic,
        );
        let line = stats.status_line(80);
        assert_eq!(line.len(), 80);
        assert!(line.contains("latency"));
        assert!(line.contains("mode"));
    }
}

use std::io::{self, Stdout, Write};
use std::os::fd::AsRawFd;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};

const DEFAULT_CELL_ASPECT_RATIO: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
    pub cell_aspect_ratio: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackLayout {
    pub terminal_rows: u16,
    pub terminal_cols: u16,
    pub render_rows: u16,
    pub render_cols: u16,
    pub offset_y: u16,
    pub offset_x: u16,
    pub status_row: u16,
    pub cell_aspect_ratio: f32,
}

impl PlaybackLayout {
    pub fn content_rows(&self) -> u16 {
        self.status_row
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    None,
    Exit,
    ToggleRenderer,
}

pub struct TerminalSession {
    stdout: Stdout,
}

impl TerminalSession {
    pub fn enter() -> Result<Self> {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode().context("failed to enable raw mode")?;
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide, Clear(ClearType::All)) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to enter alternate screen");
        }

        Ok(Self { stdout })
    }

    pub fn current_size(&self) -> Result<TerminalSize> {
        terminal_size_from_fd(self.stdout.as_raw_fd())
    }

    pub fn poll_event(&self, timeout: Duration) -> Result<TerminalEvent> {
        if !event::poll(timeout).context("failed to poll terminal events")? {
            return Ok(TerminalEvent::None);
        }

        let Event::Key(key) = event::read().context("failed to read terminal event")? else {
            return Ok(TerminalEvent::None);
        };

        Ok(classify_key_event(key.code, key.modifiers, key.kind))
    }

    pub fn render_frame(
        &mut self,
        layout: PlaybackLayout,
        frame_lines: &[String],
        status_line: &str,
    ) -> Result<()> {
        queue!(self.stdout, MoveTo(0, 0))?;

        for row in 0..layout.content_rows() {
            queue!(self.stdout, MoveTo(0, row))?;
            if row >= layout.offset_y && row < layout.offset_y + layout.render_rows {
                let source_index = usize::from(row - layout.offset_y);
                let frame_line = frame_lines
                    .get(source_index)
                    .map(String::as_str)
                    .unwrap_or("");
                let text = compose_frame_row(
                    frame_line,
                    layout.offset_x,
                    layout.render_cols,
                    layout.terminal_cols,
                );
                queue!(self.stdout, Print(text))?;
            } else {
                queue!(
                    self.stdout,
                    Print(" ".repeat(usize::from(layout.terminal_cols)))
                )?;
            }
        }

        let status = fit_status_line(status_line, layout.terminal_cols);
        queue!(
            self.stdout,
            MoveTo(0, layout.status_row),
            SetAttribute(Attribute::Reverse),
            Print(status),
            SetAttribute(Attribute::NoReverse)
        )?;

        self.stdout
            .flush()
            .context("failed to flush terminal output")
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(
            self.stdout,
            Show,
            LeaveAlternateScreen,
            SetAttribute(Attribute::Reset)
        );
        let _ = terminal::disable_raw_mode();
    }
}

pub fn terminal_size_from_fd(fd: i32) -> Result<TerminalSize> {
    let mut winsize = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let status = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut winsize) };
    if status == -1 {
        return Err(io::Error::last_os_error()).context("TIOCGWINSZ failed");
    }

    let cols = winsize.ws_col.max(1);
    let rows = winsize.ws_row.max(1);
    let cell_aspect_ratio =
        aspect_ratio_from_winsize(rows, cols, winsize.ws_xpixel, winsize.ws_ypixel);

    Ok(TerminalSize {
        rows,
        cols,
        cell_aspect_ratio,
    })
}

pub fn aspect_ratio_from_winsize(rows: u16, cols: u16, xpixels: u16, ypixels: u16) -> f32 {
    if rows == 0 || cols == 0 || xpixels == 0 || ypixels == 0 {
        return DEFAULT_CELL_ASPECT_RATIO;
    }

    let cell_width = xpixels as f32 / cols as f32;
    let cell_height = ypixels as f32 / rows as f32;
    if cell_width <= f32::EPSILON || cell_height <= f32::EPSILON {
        DEFAULT_CELL_ASPECT_RATIO
    } else {
        cell_height / cell_width
    }
}

pub fn compute_render_layout(
    size: TerminalSize,
    video_width: u32,
    video_height: u32,
) -> PlaybackLayout {
    let terminal_rows = size.rows.max(2);
    let terminal_cols = size.cols.max(1);
    let status_row = terminal_rows - 1;
    let available_rows = status_row.max(1);
    let available_cols = terminal_cols.max(1);

    let video_aspect = if video_height == 0 {
        1.0
    } else {
        video_width as f32 / video_height as f32
    };
    let target_ratio = (video_aspect * size.cell_aspect_ratio).max(0.1);

    let width_limited_rows =
        ((available_cols as f32 / target_ratio).floor() as u16).clamp(1, available_rows);
    let width_limited_cols =
        ((width_limited_rows as f32 * target_ratio).round() as u16).clamp(1, available_cols);

    let height_limited_cols =
        ((available_rows as f32 * target_ratio).floor() as u16).clamp(1, available_cols);
    let height_limited_rows =
        ((height_limited_cols as f32 / target_ratio).round() as u16).clamp(1, available_rows);

    let (render_cols, render_rows) = if area(width_limited_cols, width_limited_rows)
        >= area(height_limited_cols, height_limited_rows)
    {
        (width_limited_cols, width_limited_rows)
    } else {
        (height_limited_cols, height_limited_rows)
    };

    let offset_y = (available_rows - render_rows) / 2;
    let offset_x = (available_cols - render_cols) / 2;

    PlaybackLayout {
        terminal_rows,
        terminal_cols,
        render_rows,
        render_cols,
        offset_y,
        offset_x,
        status_row,
        cell_aspect_ratio: size.cell_aspect_ratio,
    }
}

fn area(cols: u16, rows: u16) -> u32 {
    u32::from(cols) * u32::from(rows)
}

fn compose_frame_row(
    content: &str,
    offset_col: u16,
    render_cols: u16,
    terminal_cols: u16,
) -> String {
    let width = usize::from(terminal_cols);
    let left_pad = usize::from(offset_col);
    let visible_content_width = usize::from(render_cols).min(width.saturating_sub(left_pad));
    let right_pad = width.saturating_sub(left_pad + visible_content_width);
    let mut line = String::with_capacity(left_pad + content.len() + right_pad);
    line.push_str(&" ".repeat(usize::from(offset_col)));
    line.push_str(content);
    line.push_str(&" ".repeat(right_pad));
    line
}

fn fit_status_line(status_line: &str, terminal_cols: u16) -> String {
    let width = usize::from(terminal_cols);
    let mut rendered = status_line.chars().take(width).collect::<String>();
    if rendered.len() < width {
        rendered.push_str(&" ".repeat(width - rendered.len()));
    }
    rendered
}

fn classify_key_event(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> TerminalEvent {
    if kind == KeyEventKind::Release {
        return TerminalEvent::None;
    }

    match (code, modifiers) {
        (KeyCode::Char('c' | 'C'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            TerminalEvent::Exit
        }
        (KeyCode::Char('r' | 'R'), modifiers)
            if !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT) =>
        {
            TerminalEvent::ToggleRenderer
        }
        _ => TerminalEvent::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    #[test]
    fn aspect_ratio_uses_fallback_when_pixels_missing() {
        assert_eq!(aspect_ratio_from_winsize(24, 80, 0, 0), 2.0);
        assert_eq!(aspect_ratio_from_winsize(24, 80, 800, 0), 2.0);
    }

    #[test]
    fn aspect_ratio_uses_pixel_dimensions_when_available() {
        let ratio = aspect_ratio_from_winsize(20, 100, 1000, 800);
        assert!((ratio - 4.0).abs() < 0.001);
    }

    #[test]
    fn layout_reserves_status_bar() {
        let layout = compute_render_layout(
            TerminalSize {
                rows: 40,
                cols: 120,
                cell_aspect_ratio: 2.0,
            },
            640,
            480,
        );
        assert_eq!(layout.status_row, 39);
        assert!(layout.render_rows <= 39);
    }

    #[test]
    fn layout_centers_video_when_smaller_than_terminal() {
        let layout = compute_render_layout(
            TerminalSize {
                rows: 30,
                cols: 120,
                cell_aspect_ratio: 2.0,
            },
            640,
            480,
        );
        assert!(layout.offset_x > 0);
        assert!(layout.offset_y <= layout.status_row);
    }

    #[test]
    fn compose_frame_row_preserves_ansi_sequences() {
        let content = "\x1b[38;5;240mab\x1b[0m";
        let row = compose_frame_row(content, 2, 2, 6);
        assert_eq!(row, format!("  {content}  "));
    }

    #[test]
    fn exit_key_event_matches_ctrl_c_press() {
        assert_eq!(
            classify_key_event(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            ),
            TerminalEvent::Exit
        );
        assert_eq!(
            classify_key_event(
                KeyCode::Char('C'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                KeyEventKind::Press
            ),
            TerminalEvent::Exit
        );
        assert_eq!(
            classify_key_event(KeyCode::Char('c'), KeyModifiers::NONE, KeyEventKind::Press),
            TerminalEvent::None
        );
        assert_eq!(
            classify_key_event(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                KeyEventKind::Release
            ),
            TerminalEvent::None
        );
    }

    #[test]
    fn toggle_renderer_key_event_matches_r_press() {
        assert_eq!(
            classify_key_event(KeyCode::Char('r'), KeyModifiers::NONE, KeyEventKind::Press),
            TerminalEvent::ToggleRenderer
        );
        assert_eq!(
            classify_key_event(KeyCode::Char('R'), KeyModifiers::SHIFT, KeyEventKind::Press),
            TerminalEvent::ToggleRenderer
        );
        assert_eq!(
            classify_key_event(
                KeyCode::Char('r'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            ),
            TerminalEvent::None
        );
    }
}

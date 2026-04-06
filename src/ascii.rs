use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use fontdue::{Font, FontSettings};

use crate::terminal::PlaybackLayout;
use crate::video::DecodedFrame;

const PRINTABLE_ASCII_START: u8 = 0x20;
const PRINTABLE_ASCII_END: u8 = 0x7e;
const FALLBACK_FONT_PATH: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";
const GLYPH_FONT_SIZE: f32 = 28.0;
const QUANTIZATION_BITS: u32 = 5;
const QUANTIZATION_RANGE: u32 = 1 << QUANTIZATION_BITS;
const DEFAULT_CELL_ASPECT: f32 = 2.0;

#[derive(Clone, Debug)]
pub struct SamplingCircle {
    pub center_x: f32,
    pub center_y: f32,
    pub radius: f32,
}

impl SamplingCircle {
    fn contains_normalized(&self, x: f32, y: f32) -> bool {
        let dx = x - self.center_x;
        let dy = y - self.center_y;
        dx * dx + dy * dy <= self.radius * self.radius
    }
}

#[derive(Clone, Debug)]
pub struct GlyphDescriptor {
    pub ch: char,
    pub vector: [f32; 6],
}

#[derive(Clone, Debug)]
struct GlyphPoint {
    vector: [f32; 6],
    ch: char,
}

#[derive(Clone, Debug)]
struct KdNode {
    point: GlyphPoint,
    axis: usize,
    left: Option<Box<KdNode>>,
    right: Option<Box<KdNode>>,
}

#[derive(Clone, Debug)]
struct KdTree {
    root: Option<Box<KdNode>>,
}

#[derive(Clone, Debug)]
pub struct GlyphMatcher {
    glyphs: Vec<GlyphDescriptor>,
    tree: KdTree,
    cache: [Option<char>; cache_size()],
}

#[derive(Clone, Debug)]
pub struct GlyphBank {
    cell_aspect: f32,
    circles: [SamplingCircle; 6],
    matcher: GlyphMatcher,
}

#[derive(Clone)]
pub struct Rasterizer {
    font_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RenderedFrame {
    pub rows: Vec<String>,
}

pub struct AsciiRenderer {
    rasterizer: Rasterizer,
    glyph_bank: Option<GlyphBank>,
}

#[derive(Clone, Debug)]
struct GlyphBitmap {
    width: usize,
    height: usize,
    pixels: Vec<f32>,
}

const fn cache_size() -> usize {
    1usize << (QUANTIZATION_BITS as usize * 6)
}

impl Rasterizer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            font_path: discover_monospace_font()?,
        })
    }

    pub fn font_path(&self) -> &Path {
        &self.font_path
    }

    pub fn build_bank(&self, cell_aspect: f32) -> Result<GlyphBank> {
        let aspect = if cell_aspect.is_finite() && cell_aspect > 0.0 {
            cell_aspect
        } else {
            DEFAULT_CELL_ASPECT
        };
        let font_bytes = fs::read(&self.font_path)
            .with_context(|| format!("failed to read font {}", self.font_path.display()))?;
        let font = Font::from_bytes(font_bytes, FontSettings::default())
            .map_err(|err| anyhow!("failed to parse font: {err}"))?;

        let circles = sampling_circles();
        let mut glyphs = printable_ascii()
            .map(|ch| -> Result<GlyphDescriptor> {
                let bitmap = rasterize_glyph(&font, ch, aspect)?;
                Ok(GlyphDescriptor {
                    ch,
                    vector: sample_bitmap(&bitmap, &circles),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        normalize_vectors(&mut glyphs);

        let matcher = GlyphMatcher::new(glyphs);
        Ok(GlyphBank {
            cell_aspect: aspect,
            circles,
            matcher,
        })
    }
}

impl AsciiRenderer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            rasterizer: Rasterizer::new()?,
            glyph_bank: None,
        })
    }

    pub fn rebuild_glyph_bank(&mut self, cell_aspect: f32) -> Result<()> {
        self.glyph_bank = Some(self.rasterizer.build_bank(cell_aspect)?);
        Ok(())
    }

    pub fn render_frame(&mut self, frame: &DecodedFrame, layout: &PlaybackLayout) -> RenderedFrame {
        let bank = self
            .glyph_bank
            .as_mut()
            .expect("glyph bank should be built before rendering");

        let mut rows = Vec::with_capacity(layout.render_rows as usize);
        let frame_width = frame.width as f32;
        let frame_height = frame.height as f32;

        for row in 0..layout.render_rows {
            let mut line = String::with_capacity(layout.render_cols as usize);
            let y0 = row as f32 * frame_height / layout.render_rows as f32;
            let y1 = (row as f32 + 1.0) * frame_height / layout.render_rows as f32;

            for col in 0..layout.render_cols {
                let x0 = col as f32 * frame_width / layout.render_cols as f32;
                let x1 = (col as f32 + 1.0) * frame_width / layout.render_cols as f32;
                let vector =
                    bank.sample_cell(&frame.pixels, frame.width, frame.height, x0, y0, x1, y1);
                line.push(bank.match_vector(vector));
            }

            rows.push(line);
        }

        RenderedFrame { rows }
    }
}

impl GlyphBank {
    pub fn cell_aspect(&self) -> f32 {
        self.cell_aspect
    }

    pub fn match_vector(&mut self, vector: [f32; 6]) -> char {
        self.matcher.find_best_character_quantized(vector)
    }

    pub fn sample_cell(
        &self,
        frame: &[u8],
        frame_width: usize,
        frame_height: usize,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    ) -> [f32; 6] {
        sample_frame_region(
            frame,
            frame_width,
            frame_height,
            x0,
            y0,
            x1,
            y1,
            &self.circles,
        )
    }

    pub fn glyphs(&self) -> &[GlyphDescriptor] {
        &self.matcher.glyphs
    }
}

impl GlyphMatcher {
    fn new(glyphs: Vec<GlyphDescriptor>) -> Self {
        let points = glyphs
            .iter()
            .map(|glyph| GlyphPoint {
                vector: glyph.vector,
                ch: glyph.ch,
            })
            .collect::<Vec<_>>();

        Self {
            glyphs,
            tree: KdTree::build(points),
            cache: [None; cache_size()],
        }
    }

    fn find_best_character_quantized(&mut self, vector: [f32; 6]) -> char {
        let key = quantize_to_index(vector);
        if let Some(ch) = self.cache[key] {
            return ch;
        }
        let ch = self.tree.find_nearest(vector).unwrap_or(' ');
        self.cache[key] = Some(ch);
        ch
    }
}

impl KdTree {
    fn build(mut points: Vec<GlyphPoint>) -> Self {
        let root = Self::build_node(&mut points, 0);
        Self { root }
    }

    fn build_node(points: &mut [GlyphPoint], depth: usize) -> Option<Box<KdNode>> {
        if points.is_empty() {
            return None;
        }
        let axis = depth % 6;
        points.sort_by(|a, b| {
            a.vector[axis]
                .partial_cmp(&b.vector[axis])
                .unwrap_or(Ordering::Equal)
        });
        let median = points.len() / 2;
        let (left, rest) = points.split_at_mut(median);
        let (point, right) = rest.split_first_mut()?;

        Some(Box::new(KdNode {
            point: point.clone(),
            axis,
            left: Self::build_node(left, depth + 1),
            right: Self::build_node(right, depth + 1),
        }))
    }

    fn find_nearest(&self, target: [f32; 6]) -> Option<char> {
        let mut best = None;
        let mut best_distance = f32::INFINITY;
        Self::search(&self.root, &target, &mut best, &mut best_distance);
        best
    }

    fn search(
        node: &Option<Box<KdNode>>,
        target: &[f32; 6],
        best: &mut Option<char>,
        best_distance: &mut f32,
    ) {
        let Some(node) = node else {
            return;
        };

        let distance = squared_distance(node.point.vector, *target);
        if distance < *best_distance {
            *best_distance = distance;
            *best = Some(node.point.ch);
        }

        let axis = node.axis;
        let delta = target[axis] - node.point.vector[axis];
        let (first, second) = if delta <= 0.0 {
            (&node.left, &node.right)
        } else {
            (&node.right, &node.left)
        };

        Self::search(first, target, best, best_distance);
        if delta * delta < *best_distance {
            Self::search(second, target, best, best_distance);
        }
    }
}

fn discover_monospace_font() -> Result<PathBuf> {
    let fc_match = Command::new("fc-match")
        .args(["-f", "%{file}\n", "monospace"])
        .output();

    if let Ok(output) = fc_match {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                let candidate = PathBuf::from(path);
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }

    let fallback = PathBuf::from(FALLBACK_FONT_PATH);
    if fallback.exists() {
        return Ok(fallback);
    }

    bail!("unable to locate a monospace font via fc-match or fallback path")
}

fn rasterize_glyph(font: &Font, ch: char, cell_aspect: f32) -> Result<GlyphBitmap> {
    let (metrics, bitmap) = font.rasterize(ch, GLYPH_FONT_SIZE);
    let line_metrics = font
        .horizontal_line_metrics(GLYPH_FONT_SIZE)
        .ok_or_else(|| anyhow!("font missing horizontal line metrics"))?;

    let advance = metrics.advance_width.max(1.0);
    let cell_width = advance.ceil().max(metrics.width as f32).max(1.0) as usize;
    let cell_height = line_metrics
        .new_line_size
        .ceil()
        .max(metrics.height as f32)
        .max(1.0) as usize;

    let mut pixels = vec![0.0_f32; cell_width * cell_height];
    let baseline_y = line_metrics.ascent.ceil() as i32;

    for src_y in 0..metrics.height {
        for src_x in 0..metrics.width {
            let dst_x = metrics.xmin + src_x as i32;
            let dst_y = baseline_y + metrics.ymin + src_y as i32;
            if dst_x < 0 || dst_y < 0 {
                continue;
            }
            let dst_x = dst_x as usize;
            let dst_y = dst_y as usize;
            if dst_x >= cell_width || dst_y >= cell_height {
                continue;
            }

            let src_index = src_y * metrics.width + src_x;
            let coverage = bitmap[src_index] as f32 / 255.0;
            pixels[dst_y * cell_width + dst_x] = coverage.max(pixels[dst_y * cell_width + dst_x]);
        }
    }

    let normalized_width = (cell_width as f32 / cell_aspect).max(1.0);
    let target_width = normalized_width.round().max(1.0) as usize;
    Ok(resample_bitmap(
        &GlyphBitmap {
            width: cell_width,
            height: cell_height,
            pixels,
        },
        target_width,
        cell_height,
    ))
}

fn resample_bitmap(bitmap: &GlyphBitmap, target_width: usize, target_height: usize) -> GlyphBitmap {
    if bitmap.width == target_width && bitmap.height == target_height {
        return bitmap.clone();
    }

    let mut pixels = vec![0.0_f32; target_width * target_height];
    for y in 0..target_height {
        let src_y = ((y as f32 + 0.5) / target_height as f32) * bitmap.height as f32 - 0.5;
        let src_y = src_y.clamp(0.0, (bitmap.height.saturating_sub(1)) as f32);
        let y0 = src_y.floor() as usize;
        let y1 = (y0 + 1).min(bitmap.height.saturating_sub(1));
        let wy = src_y - y0 as f32;

        for x in 0..target_width {
            let src_x = ((x as f32 + 0.5) / target_width as f32) * bitmap.width as f32 - 0.5;
            let src_x = src_x.clamp(0.0, (bitmap.width.saturating_sub(1)) as f32);
            let x0 = src_x.floor() as usize;
            let x1 = (x0 + 1).min(bitmap.width.saturating_sub(1));
            let wx = src_x - x0 as f32;

            let top = lerp(
                bitmap.pixels[y0 * bitmap.width + x0],
                bitmap.pixels[y0 * bitmap.width + x1],
                wx,
            );
            let bottom = lerp(
                bitmap.pixels[y1 * bitmap.width + x0],
                bitmap.pixels[y1 * bitmap.width + x1],
                wx,
            );
            pixels[y * target_width + x] = lerp(top, bottom, wy);
        }
    }

    GlyphBitmap {
        width: target_width,
        height: target_height,
        pixels,
    }
}

fn sample_bitmap(bitmap: &GlyphBitmap, circles: &[SamplingCircle; 6]) -> [f32; 6] {
    let mut values = [0.0_f32; 6];
    for (index, circle) in circles.iter().enumerate() {
        let mut total = 0.0;
        let mut count = 0.0;
        for y in 0..bitmap.height {
            let sample_y = (y as f32 + 0.5) / bitmap.height as f32;
            for x in 0..bitmap.width {
                let sample_x = (x as f32 + 0.5) / bitmap.width as f32;
                if circle.contains_normalized(sample_x, sample_y) {
                    total += bitmap.pixels[y * bitmap.width + x];
                    count += 1.0;
                }
            }
        }
        values[index] = if count > 0.0 { total / count } else { 0.0 };
    }
    values
}

fn sample_frame_region(
    frame: &[u8],
    frame_width: usize,
    frame_height: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    circles: &[SamplingCircle; 6],
) -> [f32; 6] {
    let mut values = [0.0_f32; 6];
    let left = x0.clamp(0.0, frame_width as f32);
    let right = x1.clamp(0.0, frame_width as f32);
    let top = y0.clamp(0.0, frame_height as f32);
    let bottom = y1.clamp(0.0, frame_height as f32);

    if right <= left || bottom <= top {
        return values;
    }

    let start_x = left.floor() as usize;
    let end_x = right.ceil().min(frame_width as f32) as usize;
    let start_y = top.floor() as usize;
    let end_y = bottom.ceil().min(frame_height as f32) as usize;

    for (index, circle) in circles.iter().enumerate() {
        let mut total = 0.0;
        let mut count = 0.0;
        for py in start_y..end_y {
            let ny = (((py as f32 + 0.5) - top) / (bottom - top)).clamp(0.0, 1.0);
            for px in start_x..end_x {
                let nx = (((px as f32 + 0.5) - left) / (right - left)).clamp(0.0, 1.0);
                if circle.contains_normalized(nx, ny) {
                    let luminance = frame[py * frame_width + px] as f32 / 255.0;
                    total += 1.0 - luminance;
                    count += 1.0;
                }
            }
        }
        values[index] = if count > 0.0 { total / count } else { 0.0 };
    }
    values
}

fn normalize_vectors(glyphs: &mut [GlyphDescriptor]) {
    let max_component = glyphs
        .iter()
        .flat_map(|glyph| glyph.vector.iter().copied())
        .fold(0.0_f32, f32::max);

    if max_component <= f32::EPSILON {
        return;
    }

    for glyph in glyphs {
        for component in &mut glyph.vector {
            *component /= max_component;
        }
    }
}

fn quantize_to_index(vector: [f32; 6]) -> usize {
    let mut key = 0usize;
    for component in vector {
        let bucket = (component.clamp(0.0, 0.999_999) * QUANTIZATION_RANGE as f32).floor() as usize;
        key = (key << QUANTIZATION_BITS) | bucket.min((QUANTIZATION_RANGE - 1) as usize);
    }
    key
}

fn squared_distance(a: [f32; 6], b: [f32; 6]) -> f32 {
    a.into_iter()
        .zip(b)
        .map(|(lhs, rhs)| {
            let delta = lhs - rhs;
            delta * delta
        })
        .sum()
}

fn sampling_circles() -> [SamplingCircle; 6] {
    let radius = 0.205;
    [
        SamplingCircle {
            center_x: 0.24,
            center_y: 0.24,
            radius,
        },
        SamplingCircle {
            center_x: 0.50,
            center_y: 0.18,
            radius,
        },
        SamplingCircle {
            center_x: 0.76,
            center_y: 0.24,
            radius,
        },
        SamplingCircle {
            center_x: 0.24,
            center_y: 0.76,
            radius,
        },
        SamplingCircle {
            center_x: 0.50,
            center_y: 0.82,
            radius,
        },
        SamplingCircle {
            center_x: 0.76,
            center_y: 0.76,
            radius,
        },
    ]
}

fn printable_ascii() -> impl Iterator<Item = char> {
    (PRINTABLE_ASCII_START..=PRINTABLE_ASCII_END).map(char::from)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[derive(Clone, Debug)]
pub struct FpsAverager {
    timestamps: VecDeque<std::time::Instant>,
    max_samples: usize,
}

impl FpsAverager {
    pub fn new(max_samples: usize) -> Self {
        Self {
            timestamps: VecDeque::with_capacity(max_samples),
            max_samples: max_samples.max(2),
        }
    }

    pub fn push(&mut self, timestamp: std::time::Instant) {
        self.timestamps.push_back(timestamp);
        while self.timestamps.len() > self.max_samples {
            self.timestamps.pop_front();
        }
    }

    pub fn fps(&self) -> f64 {
        if self.timestamps.len() < 2 {
            return 0.0;
        }
        let first = match self.timestamps.front() {
            Some(value) => *value,
            None => return 0.0,
        };
        let last = match self.timestamps.back() {
            Some(value) => *value,
            None => return 0.0,
        };
        let elapsed = last.duration_since(first).as_secs_f64();
        if elapsed <= f64::EPSILON {
            return 0.0;
        }
        (self.timestamps.len().saturating_sub(1)) as f64 / elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_stays_in_bounds() {
        let index = quantize_to_index([0.0, 0.1, 0.25, 0.5, 0.75, 0.99]);
        assert!(index < cache_size());
    }

    #[test]
    fn kd_tree_returns_exact_vector_match() {
        let glyphs = vec![
            GlyphDescriptor {
                ch: ' ',
                vector: [0.0; 6],
            },
            GlyphDescriptor {
                ch: '#',
                vector: [1.0; 6],
            },
        ];
        let mut matcher = GlyphMatcher::new(glyphs);
        assert_eq!(matcher.find_best_character_quantized([0.0; 6]), ' ');
        assert_eq!(matcher.find_best_character_quantized([1.0; 6]), '#');
    }

    #[test]
    fn fps_averager_reports_expected_rate() {
        let base = std::time::Instant::now();
        let mut fps = FpsAverager::new(8);
        fps.push(base);
        fps.push(base + std::time::Duration::from_millis(100));
        fps.push(base + std::time::Duration::from_millis(200));
        assert!((fps.fps() - 10.0).abs() < 0.001);
    }
}

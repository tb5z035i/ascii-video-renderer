use std::collections::HashMap;
use std::time::Instant;

use font8x8::{UnicodeFonts, BASIC_FONTS};

const INTERNAL_MASK_COUNT: usize = 6;
const EXTERNAL_MASK_COUNT: usize = 10;
const LOOKUP_RANGE: u32 = 8;
const GLOBAL_CONTRAST_EXPONENT: f32 = 1.55;
const DIRECTIONAL_CONTRAST_EXPONENT: f32 = 1.45;
const RESET_BYTES: &[u8] = b"\x1b[0m";

pub const DEFAULT_HARRI_CELL_WIDTH: usize = 8;
pub const DEFAULT_HARRI_CELL_ASPECT: f32 = 2.0;
pub const MIN_HARRI_CELL_HEIGHT: usize = 8;
pub const MAX_HARRI_CELL_HEIGHT: usize = 32;

const INTERNAL_CIRCLES: [SamplingCircle; INTERNAL_MASK_COUNT] = [
    SamplingCircle::new(0.24, 0.18, 0.24),
    SamplingCircle::new(0.76, 0.18, 0.24),
    SamplingCircle::new(0.18, 0.50, 0.24),
    SamplingCircle::new(0.82, 0.50, 0.24),
    SamplingCircle::new(0.24, 0.82, 0.24),
    SamplingCircle::new(0.76, 0.82, 0.24),
];

const EXTERNAL_CIRCLES: [SamplingCircle; EXTERNAL_MASK_COUNT] = [
    SamplingCircle::new(0.20, -0.12, 0.24),
    SamplingCircle::new(0.80, -0.12, 0.24),
    SamplingCircle::new(-0.12, 0.20, 0.24),
    SamplingCircle::new(1.12, 0.20, 0.24),
    SamplingCircle::new(-0.12, 0.50, 0.24),
    SamplingCircle::new(1.12, 0.50, 0.24),
    SamplingCircle::new(-0.12, 0.80, 0.24),
    SamplingCircle::new(1.12, 0.80, 0.24),
    SamplingCircle::new(0.20, 1.12, 0.24),
    SamplingCircle::new(0.80, 1.12, 0.24),
];

const AFFECTING_EXTERNAL_INDICES: [&[usize]; INTERNAL_MASK_COUNT] = [
    &[0, 1, 2, 4],
    &[0, 1, 3, 5],
    &[2, 4, 6],
    &[3, 5, 7],
    &[4, 6, 8, 9],
    &[5, 7, 8, 9],
];

#[derive(Clone, Copy, Debug)]
struct SamplingCircle {
    center_x: f32,
    center_y: f32,
    radius: f32,
}

impl SamplingCircle {
    const fn new(center_x: f32, center_y: f32, radius: f32) -> Self {
        Self {
            center_x,
            center_y,
            radius,
        }
    }

    fn contains_normalized(&self, x: f32, y: f32) -> bool {
        let dx = x - self.center_x;
        let dy = y - self.center_y;
        dx * dx + dy * dy <= self.radius * self.radius
    }
}

#[derive(Clone, Copy, Debug)]
struct FrameRegion {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

#[derive(Clone, Debug)]
struct Glyph {
    ch: u8,
    vector: [f32; INTERNAL_MASK_COUNT],
}

#[derive(Clone, Debug)]
struct GlyphBitmap {
    width: usize,
    height: usize,
    pixels: Vec<f32>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeHarriRenderStats {
    pub total_ms: f64,
    pub sample_ms: Option<f64>,
    pub lookup_ms: Option<f64>,
    pub assemble_ms: Option<f64>,
    pub sgr_change_count: u32,
    pub cache_hits: u32,
    pub cache_misses: u32,
    pub sample_count: u32,
    pub lookup_count: u32,
    pub output_bytes: u32,
}

pub struct NativeHarriRenderer {
    cell_width: usize,
    cell_height: usize,
    glyphs: Vec<Glyph>,
    cache: HashMap<u32, usize>,
    fg_sgr: Vec<Vec<u8>>,
    gray_lut: [u8; 256],
    last_output: Vec<u8>,
    last_stats: NativeHarriRenderStats,
}

impl NativeHarriRenderer {
    pub fn new(cell_width: usize, cell_height: usize) -> Result<Self, String> {
        validate_cell_dimensions(cell_width, cell_height)?;
        Ok(Self {
            cell_width,
            cell_height,
            glyphs: build_glyphs(cell_width, cell_height)?,
            cache: HashMap::new(),
            fg_sgr: build_fg_sgr(),
            gray_lut: build_gray_lut(),
            last_output: Vec::new(),
            last_stats: NativeHarriRenderStats::default(),
        })
    }

    pub fn reconfigure(&mut self, cell_width: usize, cell_height: usize) -> Result<(), String> {
        validate_cell_dimensions(cell_width, cell_height)?;
        if self.cell_width == cell_width && self.cell_height == cell_height {
            return Ok(());
        }

        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.glyphs = build_glyphs(cell_width, cell_height)?;
        self.cache.clear();
        Ok(())
    }

    pub fn cell_width(&self) -> usize {
        self.cell_width
    }

    pub fn cell_height(&self) -> usize {
        self.cell_height
    }

    pub fn render_luma(
        &mut self,
        pixels: &[u8],
        width: usize,
        height: usize,
        columns: usize,
        rows: usize,
    ) -> Result<(), String> {
        let expected_pixels = width
            .checked_mul(height)
            .ok_or_else(|| "pixel buffer length overflowed".to_string())?;
        if pixels.len() != expected_pixels {
            return Err(format!(
                "expected {expected_pixels} grayscale bytes, received {}",
                pixels.len()
            ));
        }
        if columns == 0 || rows == 0 {
            return Err("Harri output grid dimensions must be non-zero".into());
        }

        let started_at = maybe_now();
        let sample_started_at = maybe_now();
        let sampled_cells = sample_cells(pixels, width, height, columns, rows);
        let sample_ms = elapsed_ms(sample_started_at);

        let lookup_started_at = maybe_now();
        let mut output = Vec::with_capacity(
            columns
                .checked_mul(rows)
                .and_then(|value| value.checked_mul(8))
                .unwrap_or(0),
        );
        let mut cache_hits = 0u32;
        let mut cache_misses = 0u32;
        let mut sgr_change_count = 0u32;

        for row in 0..rows {
            let mut previous_fg_ansi = u8::MAX;
            for column in 0..columns {
                let cell = &sampled_cells[row * columns + column];
                let contrasted = apply_global_contrast(apply_directional_contrast(
                    cell.internal_vector,
                    cell.external_vector,
                ));
                let cache_key = quantize_vector(&contrasted);
                let glyph_index = if let Some(index) = self.cache.get(&cache_key).copied() {
                    cache_hits = cache_hits.saturating_add(1);
                    index
                } else {
                    cache_misses = cache_misses.saturating_add(1);
                    let index = find_best_glyph(&contrasted, &self.glyphs);
                    self.cache.insert(cache_key, index);
                    index
                };

                let luminance_byte = (cell.average_luminance.clamp(0.0, 1.0) * 255.0).round() as u8;
                let fg_ansi = self.gray_lut[luminance_byte as usize];
                if fg_ansi != previous_fg_ansi {
                    output.extend_from_slice(&self.fg_sgr[fg_ansi as usize]);
                    previous_fg_ansi = fg_ansi;
                    sgr_change_count = sgr_change_count.saturating_add(1);
                }
                output.push(self.glyphs[glyph_index].ch);
            }

            output.extend_from_slice(RESET_BYTES);
            if row + 1 < rows {
                output.push(b'\n');
            }
        }
        let lookup_ms = elapsed_ms(lookup_started_at);

        self.last_output = output;
        self.last_stats = NativeHarriRenderStats {
            total_ms: elapsed_ms(started_at).unwrap_or(0.0),
            sample_ms,
            lookup_ms,
            assemble_ms: None,
            sgr_change_count,
            cache_hits,
            cache_misses,
            sample_count: saturating_u32(
                columns
                    .saturating_mul(rows)
                    .saturating_mul(INTERNAL_MASK_COUNT + EXTERNAL_MASK_COUNT),
            ),
            lookup_count: saturating_u32(columns.saturating_mul(rows)),
            output_bytes: saturating_u32(self.last_output.len()),
        };
        Ok(())
    }

    pub fn output_text(&self) -> String {
        String::from_utf8_lossy(&self.last_output).into_owned()
    }

    pub fn output_bytes(&self) -> &[u8] {
        &self.last_output
    }

    pub fn stats(&self) -> NativeHarriRenderStats {
        self.last_stats
    }
}

pub fn cell_dimensions_for_aspect(cell_aspect: f32) -> (usize, usize) {
    let normalized_aspect = normalize_cell_aspect(cell_aspect);
    let cell_height =
        clamp_cell_height((DEFAULT_HARRI_CELL_WIDTH as f32 * normalized_aspect).round() as usize);
    (DEFAULT_HARRI_CELL_WIDTH, cell_height)
}

fn validate_cell_dimensions(cell_width: usize, cell_height: usize) -> Result<(), String> {
    if cell_width == 0 || cell_height == 0 {
        return Err("Harri renderer cell dimensions must be non-zero".into());
    }
    Ok(())
}

fn normalize_cell_aspect(cell_aspect: f32) -> f32 {
    if cell_aspect.is_finite() && cell_aspect > 0.0 {
        cell_aspect
    } else {
        DEFAULT_HARRI_CELL_ASPECT
    }
}

fn clamp_cell_height(cell_height: usize) -> usize {
    cell_height.clamp(MIN_HARRI_CELL_HEIGHT, MAX_HARRI_CELL_HEIGHT)
}

fn build_glyphs(cell_width: usize, cell_height: usize) -> Result<Vec<Glyph>, String> {
    let mut raw_glyphs = Vec::with_capacity(95);
    let mut max_components = [0.0_f32; INTERNAL_MASK_COUNT];

    for code_point in 0x20_u8..=0x7e {
        let ch = char::from(code_point);
        let bitmap = rasterize_glyph(ch, cell_width, cell_height)?;
        let vector = sample_bitmap(&bitmap, &INTERNAL_CIRCLES);
        for (index, value) in vector.iter().copied().enumerate() {
            max_components[index] = max_components[index].max(value);
        }
        raw_glyphs.push(Glyph {
            ch: code_point,
            vector,
        });
    }

    for glyph in &mut raw_glyphs {
        for (index, component) in glyph.vector.iter_mut().enumerate() {
            if max_components[index] > f32::EPSILON {
                *component /= max_components[index];
            }
        }
    }

    Ok(raw_glyphs)
}

fn rasterize_glyph(
    ch: char,
    target_width: usize,
    target_height: usize,
) -> Result<GlyphBitmap, String> {
    let glyph = BASIC_FONTS
        .get(ch)
        .or_else(|| BASIC_FONTS.get('?'))
        .ok_or_else(|| format!("missing bitmap font glyph for {ch:?}"))?;

    let mut pixels = vec![0.0_f32; 8 * 8];
    for (y, row_bits) in glyph.iter().copied().enumerate() {
        for x in 0..8 {
            let is_set = ((row_bits >> x) & 1) != 0;
            pixels[y * 8 + x] = if is_set { 1.0 } else { 0.0 };
        }
    }

    Ok(resample_bitmap(
        &GlyphBitmap {
            width: 8,
            height: 8,
            pixels,
        },
        target_width,
        target_height,
    ))
}

fn resample_bitmap(bitmap: &GlyphBitmap, target_width: usize, target_height: usize) -> GlyphBitmap {
    if bitmap.width == target_width && bitmap.height == target_height {
        return bitmap.clone();
    }

    let mut pixels = vec![0.0_f32; target_width.saturating_mul(target_height)];
    for y in 0..target_height {
        let src_y = ((y as f32 + 0.5) / target_height as f32) * bitmap.height as f32 - 0.5;
        let src_y = src_y.clamp(0.0, bitmap.height.saturating_sub(1) as f32);
        let y0 = src_y.floor() as usize;
        let y1 = (y0 + 1).min(bitmap.height.saturating_sub(1));
        let wy = src_y - y0 as f32;

        for x in 0..target_width {
            let src_x = ((x as f32 + 0.5) / target_width as f32) * bitmap.width as f32 - 0.5;
            let src_x = src_x.clamp(0.0, bitmap.width.saturating_sub(1) as f32);
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

fn sample_bitmap(
    bitmap: &GlyphBitmap,
    circles: &[SamplingCircle; INTERNAL_MASK_COUNT],
) -> [f32; INTERNAL_MASK_COUNT] {
    let mut values = [0.0_f32; INTERNAL_MASK_COUNT];
    for (index, circle) in circles.iter().enumerate() {
        let mut total = 0.0_f32;
        let mut count = 0.0_f32;
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

#[derive(Clone, Copy, Debug)]
struct SampledCell {
    internal_vector: [f32; INTERNAL_MASK_COUNT],
    external_vector: [f32; EXTERNAL_MASK_COUNT],
    average_luminance: f32,
}

fn sample_cells(
    pixels: &[u8],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
) -> Vec<SampledCell> {
    let mut sampled_cells = Vec::with_capacity(columns.saturating_mul(rows));
    let width_f32 = width as f32;
    let height_f32 = height as f32;

    for row in 0..rows {
        let y0 = row as f32 * height_f32 / rows as f32;
        let y1 = (row as f32 + 1.0) * height_f32 / rows as f32;
        for column in 0..columns {
            let x0 = column as f32 * width_f32 / columns as f32;
            let x1 = (column as f32 + 1.0) * width_f32 / columns as f32;
            let region = FrameRegion {
                left: x0,
                top: y0,
                right: x1,
                bottom: y1,
            };
            let internal_vector =
                sample_circle_set(pixels, width, height, region, &INTERNAL_CIRCLES);
            let external_vector =
                sample_circle_set(pixels, width, height, region, &EXTERNAL_CIRCLES);
            let average_luminance =
                internal_vector.iter().copied().sum::<f32>() / INTERNAL_MASK_COUNT as f32;
            sampled_cells.push(SampledCell {
                internal_vector,
                external_vector,
                average_luminance,
            });
        }
    }

    sampled_cells
}

fn sample_circle_set<const N: usize>(
    pixels: &[u8],
    width: usize,
    height: usize,
    region: FrameRegion,
    circles: &[SamplingCircle; N],
) -> [f32; N] {
    let mut values = [0.0_f32; N];
    let region_width = region.right - region.left;
    let region_height = region.bottom - region.top;
    if region_width <= f32::EPSILON || region_height <= f32::EPSILON {
        return values;
    }

    for (index, circle) in circles.iter().enumerate() {
        values[index] = sample_circle(
            pixels,
            width,
            height,
            region,
            region_width,
            region_height,
            circle,
        );
    }

    values
}

fn sample_circle(
    pixels: &[u8],
    width: usize,
    height: usize,
    region: FrameRegion,
    region_width: f32,
    region_height: f32,
    circle: &SamplingCircle,
) -> f32 {
    let left = region.left + (circle.center_x - circle.radius) * region_width;
    let right = region.left + (circle.center_x + circle.radius) * region_width;
    let top = region.top + (circle.center_y - circle.radius) * region_height;
    let bottom = region.top + (circle.center_y + circle.radius) * region_height;

    let start_x = left.floor().clamp(0.0, width as f32) as usize;
    let end_x = right.ceil().clamp(0.0, width as f32) as usize;
    let start_y = top.floor().clamp(0.0, height as f32) as usize;
    let end_y = bottom.ceil().clamp(0.0, height as f32) as usize;

    if start_x >= end_x || start_y >= end_y {
        return 0.0;
    }

    let mut total = 0.0_f32;
    let mut count = 0.0_f32;
    for py in start_y..end_y {
        let ny = ((py as f32 + 0.5) - region.top) / region_height;
        for px in start_x..end_x {
            let nx = ((px as f32 + 0.5) - region.left) / region_width;
            if circle.contains_normalized(nx, ny) {
                total += pixels[py * width + px] as f32 / 255.0;
                count += 1.0;
            }
        }
    }

    if count > 0.0 {
        total / count
    } else {
        0.0
    }
}

fn apply_directional_contrast(
    internal_vector: [f32; INTERNAL_MASK_COUNT],
    external_vector: [f32; EXTERNAL_MASK_COUNT],
) -> [f32; INTERNAL_MASK_COUNT] {
    let mut result = [0.0_f32; INTERNAL_MASK_COUNT];
    for index in 0..INTERNAL_MASK_COUNT {
        let mut max_value = internal_vector[index];
        for &external_index in AFFECTING_EXTERNAL_INDICES[index] {
            max_value = max_value.max(external_vector[external_index]);
        }
        result[index] = if max_value <= 0.0 {
            0.0
        } else {
            (internal_vector[index] / max_value).powf(DIRECTIONAL_CONTRAST_EXPONENT) * max_value
        };
    }
    result
}

fn apply_global_contrast(vector: [f32; INTERNAL_MASK_COUNT]) -> [f32; INTERNAL_MASK_COUNT] {
    let max_value = vector.iter().copied().fold(0.0_f32, f32::max);
    if max_value <= 0.0 {
        return [0.0_f32; INTERNAL_MASK_COUNT];
    }

    let mut result = [0.0_f32; INTERNAL_MASK_COUNT];
    for (index, value) in vector.iter().copied().enumerate() {
        result[index] = (value / max_value).powf(GLOBAL_CONTRAST_EXPONENT) * max_value;
    }
    result
}

fn quantize_vector(vector: &[f32; INTERNAL_MASK_COUNT]) -> u32 {
    let mut key = 0u32;
    for &value in vector {
        let quantized = (value * (LOOKUP_RANGE - 1) as f32).round();
        let quantized = quantized.clamp(0.0, (LOOKUP_RANGE - 1) as f32) as u32;
        key = key * LOOKUP_RANGE + quantized;
    }
    key
}

fn find_best_glyph(vector: &[f32; INTERNAL_MASK_COUNT], glyphs: &[Glyph]) -> usize {
    let mut best_index = 0usize;
    let mut best_distance = f32::INFINITY;
    for (index, glyph) in glyphs.iter().enumerate() {
        let mut distance = 0.0_f32;
        for (component, value) in vector.iter().enumerate() {
            let delta = *value - glyph.vector[component];
            distance += delta * delta;
        }
        if distance < best_distance {
            best_distance = distance;
            best_index = index;
        }
    }
    best_index
}

fn build_palette() -> Vec<[u8; 3]> {
    let mut palette = Vec::with_capacity(256);
    let system_colors = [
        [0, 0, 0],
        [128, 0, 0],
        [0, 128, 0],
        [128, 128, 0],
        [0, 0, 128],
        [128, 0, 128],
        [0, 128, 128],
        [192, 192, 192],
        [128, 128, 128],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [0, 0, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    palette.extend(system_colors);

    let cube_steps = [0, 95, 135, 175, 215, 255];
    for &r in &cube_steps {
        for &g in &cube_steps {
            for &b in &cube_steps {
                palette.push([r, g, b]);
            }
        }
    }

    for value in 0..24u8 {
        let gray = 8u8.saturating_add(value.saturating_mul(10));
        palette.push([gray, gray, gray]);
    }

    palette
}

fn build_gray_lut() -> [u8; 256] {
    let palette = build_palette();
    let mut lut = [16u8; 256];
    for gray in 0..=255u16 {
        let gray = gray as i32;
        let mut best_distance = i32::MAX;
        let mut best_index = 16u8;
        for (index, color) in palette.iter().enumerate().skip(16) {
            let dr = gray - color[0] as i32;
            let dg = gray - color[1] as i32;
            let db = gray - color[2] as i32;
            let distance = dr * dr + dg * dg + db * db;
            if distance < best_distance {
                best_distance = distance;
                best_index = index as u8;
            }
        }
        lut[gray as usize] = best_index;
    }
    lut
}

fn build_fg_sgr() -> Vec<Vec<u8>> {
    (0..=255)
        .map(|index| format!("\x1b[38;5;{index}m").into_bytes())
        .collect()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn maybe_now() -> Option<Instant> {
    if cfg!(target_arch = "wasm32") {
        None
    } else {
        Some(Instant::now())
    }
}

fn elapsed_ms(started_at: Option<Instant>) -> Option<f64> {
    started_at.map(|value| value.elapsed().as_secs_f64() * 1_000.0)
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_dimensions_follow_terminal_aspect_ratio() {
        assert_eq!(cell_dimensions_for_aspect(2.0), (8, 16));
        assert_eq!(cell_dimensions_for_aspect(3.0), (8, 24));
        assert_eq!(cell_dimensions_for_aspect(0.0), (8, 16));
    }

    #[test]
    fn harri_renderer_emits_visible_output() {
        let mut renderer = NativeHarriRenderer::new(8, 16).expect("renderer should initialize");
        let pixels = vec![255u8; 16 * 12];
        renderer
            .render_luma(&pixels, 16, 12, 4, 3)
            .expect("render should succeed");
        let output = renderer.output_text();
        assert!(output.contains("\x1b[38;5;"));
        assert!(output.ends_with("\x1b[0m"));
    }
}

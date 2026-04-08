use std::time::Instant;

const RESET_BYTES: &[u8] = b"\x1b[0m";
const SHADE_GLYPHS: [char; 5] = [' ', '░', '▒', '▓', '█'];
const SEXTANT_LEFT_HALF_MASK: u8 = 0b01_0101;
const SEXTANT_RIGHT_HALF_MASK: u8 = 0b10_1010;
const SEXTANT_FULL_MASK: u8 = 0b11_1111;
const SEXTANT_ON_THRESHOLD: u8 = 128;

#[derive(Clone, Copy, Debug, Default)]
pub struct UnicodeBlocksRenderStats {
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

pub struct UnicodeBlocksRenderer {
    cell_width: usize,
    cell_height: usize,
    fg_sgr: Vec<Vec<u8>>,
    gray_lut: [u8; 256],
    last_output: Vec<u8>,
    last_stats: UnicodeBlocksRenderStats,
}

#[derive(Clone, Copy, Debug)]
struct FrameRegion {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

#[derive(Clone, Copy, Debug)]
struct SampledSextantGrayCell {
    mask: u8,
    avg_luma: u8,
}

#[derive(Clone, Copy, Debug)]
struct SampledSextantRgbCell {
    mask: u8,
    avg_rgb: [u8; 3],
}

#[derive(Clone, Copy, Debug)]
struct SampledShadeGrayCell {
    avg_luma: u8,
}

#[derive(Clone, Copy, Debug)]
struct SampledShadeRgbCell {
    avg_luma: u8,
    avg_rgb: [u8; 3],
}

impl UnicodeBlocksRenderer {
    pub fn new(cell_width: usize, cell_height: usize) -> Result<Self, String> {
        validate_cell_dimensions(cell_width, cell_height)?;
        Ok(Self {
            cell_width,
            cell_height,
            fg_sgr: build_fg_sgr(),
            gray_lut: build_gray_lut(),
            last_output: Vec::new(),
            last_stats: UnicodeBlocksRenderStats::default(),
        })
    }

    pub fn reconfigure(&mut self, cell_width: usize, cell_height: usize) -> Result<(), String> {
        validate_cell_dimensions(cell_width, cell_height)?;
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        Ok(())
    }

    pub fn cell_width(&self) -> usize {
        self.cell_width
    }

    pub fn cell_height(&self) -> usize {
        self.cell_height
    }

    pub fn render_sextant_luma(
        &mut self,
        pixels: &[u8],
        width: usize,
        height: usize,
        columns: usize,
        rows: usize,
    ) -> Result<(), String> {
        validate_grayscale_input(pixels, width, height, columns, rows)?;

        let started_at = maybe_now();
        let sample_started_at = maybe_now();
        let sampled_cells = sample_sextant_cells_gray(pixels, width, height, columns, rows);
        let sample_ms = elapsed_ms(sample_started_at);
        let assemble_started_at = maybe_now();

        let mut output = Vec::with_capacity(
            columns
                .saturating_mul(rows)
                .saturating_mul(8)
                .saturating_add(rows),
        );
        let mut sgr_change_count = 0u32;

        for row in 0..rows {
            let mut previous_fg_ansi: Option<u8> = None;
            for column in 0..columns {
                let cell = sampled_cells[row * columns + column];
                let glyph = sextant_char(cell.mask);
                if glyph != ' ' {
                    let fg_ansi = self.gray_lut[cell.avg_luma as usize];
                    if previous_fg_ansi != Some(fg_ansi) {
                        output.extend_from_slice(&self.fg_sgr[fg_ansi as usize]);
                        previous_fg_ansi = Some(fg_ansi);
                        sgr_change_count = sgr_change_count.saturating_add(1);
                    }
                }
                push_char(&mut output, glyph);
            }

            output.extend_from_slice(RESET_BYTES);
            if row + 1 < rows {
                output.push(b'\n');
            }
        }

        self.last_output = output;
        self.last_stats = UnicodeBlocksRenderStats {
            total_ms: elapsed_ms(started_at).unwrap_or(0.0),
            sample_ms,
            lookup_ms: None,
            assemble_ms: elapsed_ms(assemble_started_at),
            sgr_change_count,
            cache_hits: 0,
            cache_misses: 0,
            sample_count: saturating_u32(columns.saturating_mul(rows).saturating_mul(6)),
            lookup_count: saturating_u32(columns.saturating_mul(rows)),
            output_bytes: saturating_u32(self.last_output.len()),
        };
        Ok(())
    }

    pub fn render_sextant_rgb(
        &mut self,
        pixels: &[u8],
        width: usize,
        height: usize,
        columns: usize,
        rows: usize,
    ) -> Result<(), String> {
        validate_rgb_input(pixels, width, height, columns, rows)?;

        let started_at = maybe_now();
        let sample_started_at = maybe_now();
        let sampled_cells = sample_sextant_cells_rgb(pixels, width, height, columns, rows);
        let sample_ms = elapsed_ms(sample_started_at);
        let assemble_started_at = maybe_now();

        let mut output = Vec::with_capacity(
            columns
                .saturating_mul(rows)
                .saturating_mul(18)
                .saturating_add(rows),
        );
        let mut sgr_change_count = 0u32;

        for row in 0..rows {
            let mut previous_color: Option<[u8; 3]> = None;
            for column in 0..columns {
                let cell = sampled_cells[row * columns + column];
                let glyph = sextant_char(cell.mask);
                if glyph != ' ' && previous_color != Some(cell.avg_rgb) {
                    push_truecolor_fg(
                        &mut output,
                        cell.avg_rgb[0],
                        cell.avg_rgb[1],
                        cell.avg_rgb[2],
                    );
                    previous_color = Some(cell.avg_rgb);
                    sgr_change_count = sgr_change_count.saturating_add(1);
                }
                push_char(&mut output, glyph);
            }

            output.extend_from_slice(RESET_BYTES);
            if row + 1 < rows {
                output.push(b'\n');
            }
        }

        self.last_output = output;
        self.last_stats = UnicodeBlocksRenderStats {
            total_ms: elapsed_ms(started_at).unwrap_or(0.0),
            sample_ms,
            lookup_ms: None,
            assemble_ms: elapsed_ms(assemble_started_at),
            sgr_change_count,
            cache_hits: 0,
            cache_misses: 0,
            sample_count: saturating_u32(columns.saturating_mul(rows).saturating_mul(6)),
            lookup_count: saturating_u32(columns.saturating_mul(rows)),
            output_bytes: saturating_u32(self.last_output.len()),
        };
        Ok(())
    }

    pub fn render_shade_blocks_luma(
        &mut self,
        pixels: &[u8],
        width: usize,
        height: usize,
        columns: usize,
        rows: usize,
    ) -> Result<(), String> {
        validate_grayscale_input(pixels, width, height, columns, rows)?;

        let started_at = maybe_now();
        let sample_started_at = maybe_now();
        let sampled_cells = sample_shade_cells_gray(pixels, width, height, columns, rows);
        let sample_ms = elapsed_ms(sample_started_at);
        let assemble_started_at = maybe_now();

        let mut output = Vec::with_capacity(
            columns
                .saturating_mul(rows)
                .saturating_mul(8)
                .saturating_add(rows),
        );
        let mut sgr_change_count = 0u32;

        for row in 0..rows {
            let mut previous_fg_ansi: Option<u8> = None;
            for column in 0..columns {
                let cell = sampled_cells[row * columns + column];
                let glyph = shade_char(cell.avg_luma);
                if glyph != ' ' {
                    let fg_ansi = self.gray_lut[cell.avg_luma as usize];
                    if previous_fg_ansi != Some(fg_ansi) {
                        output.extend_from_slice(&self.fg_sgr[fg_ansi as usize]);
                        previous_fg_ansi = Some(fg_ansi);
                        sgr_change_count = sgr_change_count.saturating_add(1);
                    }
                }
                push_char(&mut output, glyph);
            }

            output.extend_from_slice(RESET_BYTES);
            if row + 1 < rows {
                output.push(b'\n');
            }
        }

        self.last_output = output;
        self.last_stats = UnicodeBlocksRenderStats {
            total_ms: elapsed_ms(started_at).unwrap_or(0.0),
            sample_ms,
            lookup_ms: None,
            assemble_ms: elapsed_ms(assemble_started_at),
            sgr_change_count,
            cache_hits: 0,
            cache_misses: 0,
            sample_count: saturating_u32(columns.saturating_mul(rows)),
            lookup_count: saturating_u32(columns.saturating_mul(rows)),
            output_bytes: saturating_u32(self.last_output.len()),
        };
        Ok(())
    }

    pub fn render_shade_blocks_rgb(
        &mut self,
        pixels: &[u8],
        width: usize,
        height: usize,
        columns: usize,
        rows: usize,
    ) -> Result<(), String> {
        validate_rgb_input(pixels, width, height, columns, rows)?;

        let started_at = maybe_now();
        let sample_started_at = maybe_now();
        let sampled_cells = sample_shade_cells_rgb(pixels, width, height, columns, rows);
        let sample_ms = elapsed_ms(sample_started_at);
        let assemble_started_at = maybe_now();

        let mut output = Vec::with_capacity(
            columns
                .saturating_mul(rows)
                .saturating_mul(18)
                .saturating_add(rows),
        );
        let mut sgr_change_count = 0u32;

        for row in 0..rows {
            let mut previous_color: Option<[u8; 3]> = None;
            for column in 0..columns {
                let cell = sampled_cells[row * columns + column];
                let glyph = shade_char(cell.avg_luma);
                if glyph != ' ' && previous_color != Some(cell.avg_rgb) {
                    push_truecolor_fg(
                        &mut output,
                        cell.avg_rgb[0],
                        cell.avg_rgb[1],
                        cell.avg_rgb[2],
                    );
                    previous_color = Some(cell.avg_rgb);
                    sgr_change_count = sgr_change_count.saturating_add(1);
                }
                push_char(&mut output, glyph);
            }

            output.extend_from_slice(RESET_BYTES);
            if row + 1 < rows {
                output.push(b'\n');
            }
        }

        self.last_output = output;
        self.last_stats = UnicodeBlocksRenderStats {
            total_ms: elapsed_ms(started_at).unwrap_or(0.0),
            sample_ms,
            lookup_ms: None,
            assemble_ms: elapsed_ms(assemble_started_at),
            sgr_change_count,
            cache_hits: 0,
            cache_misses: 0,
            sample_count: saturating_u32(columns.saturating_mul(rows)),
            lookup_count: saturating_u32(columns.saturating_mul(rows)),
            output_bytes: saturating_u32(self.last_output.len()),
        };
        Ok(())
    }

    pub fn output_text(&self) -> String {
        String::from_utf8_lossy(&self.last_output).into_owned()
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn output_bytes(&self) -> &[u8] {
        &self.last_output
    }

    pub fn stats(&self) -> UnicodeBlocksRenderStats {
        self.last_stats
    }
}

fn validate_cell_dimensions(cell_width: usize, cell_height: usize) -> Result<(), String> {
    if cell_width == 0 || cell_height == 0 {
        return Err("unicode-block renderer cell dimensions must be non-zero".into());
    }
    Ok(())
}

fn validate_grayscale_input(
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
        return Err("unicode-block output grid dimensions must be non-zero".into());
    }
    Ok(())
}

fn validate_rgb_input(
    pixels: &[u8],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
) -> Result<(), String> {
    let expected_pixels = width
        .checked_mul(height)
        .and_then(|value| value.checked_mul(3))
        .ok_or_else(|| "pixel buffer length overflowed".to_string())?;
    if pixels.len() != expected_pixels {
        return Err(format!(
            "expected {expected_pixels} rgb24 bytes, received {}",
            pixels.len()
        ));
    }
    if columns == 0 || rows == 0 {
        return Err("unicode-block output grid dimensions must be non-zero".into());
    }
    Ok(())
}

fn sample_sextant_cells_gray(
    pixels: &[u8],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
) -> Vec<SampledSextantGrayCell> {
    let mut sampled_cells = Vec::with_capacity(columns.saturating_mul(rows));
    let width_f32 = width as f32;
    let height_f32 = height as f32;

    for row in 0..rows {
        let y0 = row as f32 * height_f32 / rows as f32;
        let y1 = (row as f32 + 1.0) * height_f32 / rows as f32;
        for column in 0..columns {
            let x0 = column as f32 * width_f32 / columns as f32;
            let x1 = (column as f32 + 1.0) * width_f32 / columns as f32;
            sampled_cells.push(sample_sextant_cell_gray(
                pixels,
                width,
                height,
                FrameRegion {
                    left: x0,
                    top: y0,
                    right: x1,
                    bottom: y1,
                },
            ));
        }
    }

    sampled_cells
}

fn sample_sextant_cells_rgb(
    pixels: &[u8],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
) -> Vec<SampledSextantRgbCell> {
    let mut sampled_cells = Vec::with_capacity(columns.saturating_mul(rows));
    let width_f32 = width as f32;
    let height_f32 = height as f32;

    for row in 0..rows {
        let y0 = row as f32 * height_f32 / rows as f32;
        let y1 = (row as f32 + 1.0) * height_f32 / rows as f32;
        for column in 0..columns {
            let x0 = column as f32 * width_f32 / columns as f32;
            let x1 = (column as f32 + 1.0) * width_f32 / columns as f32;
            sampled_cells.push(sample_sextant_cell_rgb(
                pixels,
                width,
                height,
                FrameRegion {
                    left: x0,
                    top: y0,
                    right: x1,
                    bottom: y1,
                },
            ));
        }
    }

    sampled_cells
}

fn sample_shade_cells_gray(
    pixels: &[u8],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
) -> Vec<SampledShadeGrayCell> {
    let mut sampled_cells = Vec::with_capacity(columns.saturating_mul(rows));
    let width_f32 = width as f32;
    let height_f32 = height as f32;

    for row in 0..rows {
        let y0 = row as f32 * height_f32 / rows as f32;
        let y1 = (row as f32 + 1.0) * height_f32 / rows as f32;
        for column in 0..columns {
            let x0 = column as f32 * width_f32 / columns as f32;
            let x1 = (column as f32 + 1.0) * width_f32 / columns as f32;
            sampled_cells.push(SampledShadeGrayCell {
                avg_luma: sample_gray_region(
                    pixels,
                    width,
                    height,
                    FrameRegion {
                        left: x0,
                        top: y0,
                        right: x1,
                        bottom: y1,
                    },
                ),
            });
        }
    }

    sampled_cells
}

fn sample_shade_cells_rgb(
    pixels: &[u8],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
) -> Vec<SampledShadeRgbCell> {
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
            let avg_rgb = sample_rgb_region(pixels, width, height, region);
            sampled_cells.push(SampledShadeRgbCell {
                avg_luma: luma601_u8(avg_rgb[0], avg_rgb[1], avg_rgb[2]),
                avg_rgb,
            });
        }
    }

    sampled_cells
}

fn sample_sextant_cell_gray(
    pixels: &[u8],
    width: usize,
    height: usize,
    region: FrameRegion,
) -> SampledSextantGrayCell {
    let Some((left, right, top, bottom)) = clamped_bounds(region, width, height) else {
        return SampledSextantGrayCell {
            mask: 0,
            avg_luma: 0,
        };
    };

    let region_width = region.right - region.left;
    let region_height = region.bottom - region.top;
    if region_width <= f32::EPSILON || region_height <= f32::EPSILON {
        return SampledSextantGrayCell {
            mask: 0,
            avg_luma: 0,
        };
    }

    let mut total_sum = 0u32;
    let mut total_count = 0u32;
    let mut sextant_sums = [0u32; 6];
    let mut sextant_counts = [0u32; 6];

    for py in top..bottom {
        let ny = (((py as f32 + 0.5) - region.top) / region_height).clamp(0.0, 0.999_999);
        let y_bucket = sextant_y_bucket(ny);
        let row_base = py * width;
        for px in left..right {
            let nx = (((px as f32 + 0.5) - region.left) / region_width).clamp(0.0, 0.999_999);
            let x_bucket = sextant_x_bucket(nx);
            let sextant_index = y_bucket * 2 + x_bucket;
            let value = u32::from(pixels[row_base + px]);
            total_sum += value;
            total_count += 1;
            sextant_sums[sextant_index] += value;
            sextant_counts[sextant_index] += 1;
        }
    }

    let mut mask = 0u8;
    for index in 0..6 {
        let average = average_u8(sextant_sums[index], sextant_counts[index]);
        if average >= SEXTANT_ON_THRESHOLD {
            mask |= 1 << index;
        }
    }

    SampledSextantGrayCell {
        mask,
        avg_luma: average_u8(total_sum, total_count),
    }
}

fn sample_sextant_cell_rgb(
    pixels: &[u8],
    width: usize,
    height: usize,
    region: FrameRegion,
) -> SampledSextantRgbCell {
    let Some((left, right, top, bottom)) = clamped_bounds(region, width, height) else {
        return SampledSextantRgbCell {
            mask: 0,
            avg_rgb: [0, 0, 0],
        };
    };

    let region_width = region.right - region.left;
    let region_height = region.bottom - region.top;
    if region_width <= f32::EPSILON || region_height <= f32::EPSILON {
        return SampledSextantRgbCell {
            mask: 0,
            avg_rgb: [0, 0, 0],
        };
    }

    let mut total_r = 0u32;
    let mut total_g = 0u32;
    let mut total_b = 0u32;
    let mut total_count = 0u32;
    let mut sextant_luma_sums = [0u32; 6];
    let mut sextant_counts = [0u32; 6];

    for py in top..bottom {
        let ny = (((py as f32 + 0.5) - region.top) / region_height).clamp(0.0, 0.999_999);
        let y_bucket = sextant_y_bucket(ny);
        let row_base = py * width * 3;
        for px in left..right {
            let nx = (((px as f32 + 0.5) - region.left) / region_width).clamp(0.0, 0.999_999);
            let x_bucket = sextant_x_bucket(nx);
            let sextant_index = y_bucket * 2 + x_bucket;
            let pixel_index = row_base + px * 3;
            let r = pixels[pixel_index];
            let g = pixels[pixel_index + 1];
            let b = pixels[pixel_index + 2];
            total_r += u32::from(r);
            total_g += u32::from(g);
            total_b += u32::from(b);
            total_count += 1;
            sextant_luma_sums[sextant_index] += u32::from(luma601_u8(r, g, b));
            sextant_counts[sextant_index] += 1;
        }
    }

    let mut mask = 0u8;
    for index in 0..6 {
        let average = average_u8(sextant_luma_sums[index], sextant_counts[index]);
        if average >= SEXTANT_ON_THRESHOLD {
            mask |= 1 << index;
        }
    }

    SampledSextantRgbCell {
        mask,
        avg_rgb: [
            average_u8(total_r, total_count),
            average_u8(total_g, total_count),
            average_u8(total_b, total_count),
        ],
    }
}

fn sample_gray_region(pixels: &[u8], width: usize, height: usize, region: FrameRegion) -> u8 {
    let Some((left, right, top, bottom)) = clamped_bounds(region, width, height) else {
        return 0;
    };

    let mut sum = 0u32;
    let mut count = 0u32;
    for py in top..bottom {
        let row_base = py * width;
        for px in left..right {
            sum += u32::from(pixels[row_base + px]);
            count += 1;
        }
    }

    average_u8(sum, count)
}

fn sample_rgb_region(pixels: &[u8], width: usize, height: usize, region: FrameRegion) -> [u8; 3] {
    let Some((left, right, top, bottom)) = clamped_bounds(region, width, height) else {
        return [0, 0, 0];
    };

    let mut sum_r = 0u32;
    let mut sum_g = 0u32;
    let mut sum_b = 0u32;
    let mut count = 0u32;
    for py in top..bottom {
        let row_base = py * width * 3;
        for px in left..right {
            let pixel_index = row_base + px * 3;
            sum_r += u32::from(pixels[pixel_index]);
            sum_g += u32::from(pixels[pixel_index + 1]);
            sum_b += u32::from(pixels[pixel_index + 2]);
            count += 1;
        }
    }

    [
        average_u8(sum_r, count),
        average_u8(sum_g, count),
        average_u8(sum_b, count),
    ]
}

fn clamped_bounds(
    region: FrameRegion,
    width: usize,
    height: usize,
) -> Option<(usize, usize, usize, usize)> {
    let left = region.left.floor().clamp(0.0, width as f32) as usize;
    let right = region.right.ceil().clamp(0.0, width as f32) as usize;
    let top = region.top.floor().clamp(0.0, height as f32) as usize;
    let bottom = region.bottom.ceil().clamp(0.0, height as f32) as usize;
    if left >= right || top >= bottom {
        None
    } else {
        Some((left, right, top, bottom))
    }
}

fn sextant_x_bucket(nx: f32) -> usize {
    if nx < 0.5 {
        0
    } else {
        1
    }
}

fn sextant_y_bucket(ny: f32) -> usize {
    if ny < (1.0 / 3.0) {
        0
    } else if ny < (2.0 / 3.0) {
        1
    } else {
        2
    }
}

fn average_u8(sum: u32, count: u32) -> u8 {
    if count == 0 {
        0
    } else {
        ((sum + count / 2) / count).min(255) as u8
    }
}

fn luma601_u8(r: u8, g: u8, b: u8) -> u8 {
    ((299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b) + 500) / 1000) as u8
}

fn sextant_char(mask: u8) -> char {
    match mask {
        0 => ' ',
        SEXTANT_LEFT_HALF_MASK => '▌',
        SEXTANT_RIGHT_HALF_MASK => '▐',
        SEXTANT_FULL_MASK => '█',
        _ => {
            let mut offset = u32::from(mask - 1);
            if mask > SEXTANT_LEFT_HALF_MASK {
                offset -= 1;
            }
            if mask > SEXTANT_RIGHT_HALF_MASK {
                offset -= 1;
            }
            char::from_u32(0x1FB00 + offset).unwrap_or(' ')
        }
    }
}

fn shade_char(luma: u8) -> char {
    let levels = SHADE_GLYPHS.len() - 1;
    let index = (usize::from(luma) * levels + 127) / 255;
    SHADE_GLYPHS[index]
}

fn push_char(output: &mut Vec<u8>, ch: char) {
    let mut buffer = [0u8; 4];
    output.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
}

pub(crate) fn push_truecolor_fg(buf: &mut Vec<u8>, r: u8, g: u8, b: u8) {
    buf.extend_from_slice(b"\x1b[38;2;");
    push_decimal_u8(buf, r);
    buf.push(b';');
    push_decimal_u8(buf, g);
    buf.push(b';');
    push_decimal_u8(buf, b);
    buf.push(b'm');
}

pub(crate) fn push_truecolor_bg(buf: &mut Vec<u8>, r: u8, g: u8, b: u8) {
    buf.extend_from_slice(b"\x1b[48;2;");
    push_decimal_u8(buf, r);
    buf.push(b';');
    push_decimal_u8(buf, g);
    buf.push(b';');
    push_decimal_u8(buf, b);
    buf.push(b'm');
}

fn push_decimal_u8(buf: &mut Vec<u8>, mut n: u8) {
    if n >= 100 {
        buf.push(b'0' + n / 100);
        n %= 100;
        buf.push(b'0' + n / 10);
        buf.push(b'0' + n % 10);
    } else if n >= 10 {
        buf.push(b'0' + n / 10);
        buf.push(b'0' + n % 10);
    } else {
        buf.push(b'0' + n);
    }
}

fn build_palette() -> Vec<[u8; 3]> {
    let mut palette = Vec::with_capacity(256);
    palette.extend([
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
    ]);

    for &r in &[0, 95, 135, 175, 215, 255] {
        for &g in &[0, 95, 135, 175, 215, 255] {
            for &b in &[0, 95, 135, 175, 215, 255] {
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

pub(crate) fn build_gray_lut() -> [u8; 256] {
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

pub(crate) fn build_fg_sgr() -> Vec<Vec<u8>> {
    (0..=255)
        .map(|index| format!("\x1b[38;5;{index}m").into_bytes())
        .collect()
}

pub(crate) fn maybe_now() -> Option<Instant> {
    if cfg!(target_arch = "wasm32") {
        None
    } else {
        Some(Instant::now())
    }
}

pub(crate) fn elapsed_ms(started_at: Option<Instant>) -> Option<f64> {
    started_at.map(|value| value.elapsed().as_secs_f64() * 1_000.0)
}

pub(crate) fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sextant_mapping_uses_expected_unicode_positions() {
        assert_eq!(sextant_char(0), ' ');
        assert_eq!(sextant_char(0b00_0001), '\u{1fb00}');
        assert_eq!(sextant_char(0b00_0011), '\u{1fb02}');
        assert_eq!(sextant_char(SEXTANT_LEFT_HALF_MASK), '▌');
        assert_eq!(sextant_char(SEXTANT_RIGHT_HALF_MASK), '▐');
        assert_eq!(sextant_char(SEXTANT_FULL_MASK), '█');
    }

    #[test]
    fn sextant_renderer_respects_bit_order() {
        let mut renderer =
            UnicodeBlocksRenderer::new(8, 16).expect("unicode-block renderer should initialize");
        let pixels = vec![
            255, 0, //
            0, 0, //
            0, 0, //
        ];
        renderer
            .render_sextant_luma(&pixels, 2, 3, 1, 1)
            .expect("sextant grayscale render should succeed");
        assert!(renderer.output_text().contains('\u{1fb00}'));
    }

    #[test]
    fn sextant_renderer_rgb_emits_truecolor_sequences() {
        let mut renderer =
            UnicodeBlocksRenderer::new(8, 16).expect("unicode-block renderer should initialize");
        let pixels = vec![
            255, 0, 0, 0, 255, 0, //
            0, 0, 255, 0, 0, 0, //
            0, 0, 0, 255, 255, 255, //
        ];
        renderer
            .render_sextant_rgb(&pixels, 2, 3, 1, 1)
            .expect("sextant rgb render should succeed");
        let output = renderer.output_text();
        assert!(output.contains("\x1b[38;2;"));
        assert!(output.ends_with("\x1b[0m"));
    }

    #[test]
    fn shade_renderer_grayscale_emits_shade_blocks() {
        let mut renderer =
            UnicodeBlocksRenderer::new(8, 16).expect("unicode-block renderer should initialize");
        let pixels = vec![255u8; 4];
        renderer
            .render_shade_blocks_luma(&pixels, 2, 2, 1, 1)
            .expect("shade grayscale render should succeed");
        let output = renderer.output_text();
        assert!(output.contains('█'));
        assert!(output.contains("\x1b[38;5;"));
    }

    #[test]
    fn shade_renderer_rgb_emits_truecolor_sequences() {
        let mut renderer =
            UnicodeBlocksRenderer::new(8, 16).expect("unicode-block renderer should initialize");
        let pixels = vec![
            255, 0, 0, 255, 0, 0, //
            255, 0, 0, 255, 0, 0, //
        ];
        renderer
            .render_shade_blocks_rgb(&pixels, 2, 2, 1, 1)
            .expect("shade rgb render should succeed");
        let output = renderer.output_text();
        assert!(output.contains("\x1b[38;2;"));
        assert!(output.chars().any(|ch| ['░', '▒', '▓', '█'].contains(&ch)));
        assert!(output.ends_with("\x1b[0m"));
    }
}

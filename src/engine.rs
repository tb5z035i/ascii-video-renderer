use anyhow::{anyhow, bail, Result};

use crate::ascii::{AsciiGrid, AsciiRenderer};
use crate::context_shape::{cell_dimensions_for_aspect, ContextShapeRenderer};
use crate::unicode_blocks::{UnicodeBlocksRenderStats, UnicodeBlocksRenderer};

const DEFAULT_CELL_ASPECT: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderAlgorithm {
    /// Samples and matches glyphs using only pixels inside each output cell; glyphs from the
    /// system monospace font (see `ascii` module).
    LocalShape,
    /// Samples a band outside each cell for context, matches against an embedded bitmap font.
    ContextShape,
    /// Like [`Self::ContextShape`], but expects RGB24 frames and draws ANSI truecolor (`38;2;…`)
    /// foreground per cell.
    ContextShapeColor,
    /// Samples the top and bottom halves of each cell from RGB24 input and renders a truecolor
    /// half-block glyph using foreground/background ANSI colors.
    HalfBlockColor,
    /// Samples each output cell as a 2x3 raster and emits a Unicode sextant glyph in grayscale.
    Sextant,
    /// Like [`Self::Sextant`], but colors the sextant glyph with ANSI truecolor foreground.
    SextantColor,
    /// Quantizes each cell to Unicode shade blocks (`░▒▓█`) using grayscale ANSI foreground.
    ShadeBlocks,
    /// Like [`Self::ShadeBlocks`], but colors the shade glyph with ANSI truecolor foreground.
    ShadeBlocksColor,
}

impl RenderAlgorithm {
    pub fn id(self) -> &'static str {
        match self {
            Self::LocalShape => "local_shape",
            Self::ContextShape => "context_shape",
            Self::ContextShapeColor => "context_shape_color",
            Self::HalfBlockColor => "half_block_color",
            Self::Sextant => "sextant",
            Self::SextantColor => "sextant_color",
            Self::ShadeBlocks => "shade_blocks",
            Self::ShadeBlocksColor => "shade_blocks_color",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LocalShape => "Local",
            Self::ContextShape => "Context",
            Self::ContextShapeColor => "Color",
            Self::HalfBlockColor => "HalfBlk",
            Self::Sextant => "Sextant",
            Self::SextantColor => "SextRGB",
            Self::ShadeBlocks => "Shade",
            Self::ShadeBlocksColor => "ShdRGB",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::LocalShape => Self::ContextShape,
            Self::ContextShape => Self::ContextShapeColor,
            Self::ContextShapeColor => Self::HalfBlockColor,
            Self::HalfBlockColor => Self::Sextant,
            Self::Sextant => Self::SextantColor,
            Self::SextantColor => Self::ShadeBlocks,
            Self::ShadeBlocks => Self::ShadeBlocksColor,
            Self::ShadeBlocksColor => Self::LocalShape,
        }
    }

    pub fn needs_rgb_frames(self) -> bool {
        matches!(
            self,
            Self::ContextShapeColor
                | Self::HalfBlockColor
                | Self::SextantColor
                | Self::ShadeBlocksColor
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct EngineRenderTimings {
    pub total_ms: f64,
    pub sample_ms: Option<f64>,
    pub lookup_ms: Option<f64>,
    pub assemble_ms: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct EngineRenderStats {
    pub sample_count: usize,
    pub lookup_count: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub cell_count: usize,
    pub output_bytes: usize,
    pub sgr_change_count: Option<usize>,
    pub timings: EngineRenderTimings,
}

#[derive(Clone, Debug)]
pub struct EngineRenderedFrame {
    pub rows: Vec<String>,
    pub stats: EngineRenderStats,
}

enum EngineInner {
    LocalShape(AsciiRenderer),
    ContextShape(Box<ContextShapeRenderer>),
    ContextShapeColor(Box<ContextShapeRenderer>),
    HalfBlockColor(Box<ContextShapeRenderer>),
    Sextant(Box<UnicodeBlocksRenderer>),
    SextantColor(Box<UnicodeBlocksRenderer>),
    ShadeBlocks(Box<UnicodeBlocksRenderer>),
    ShadeBlocksColor(Box<UnicodeBlocksRenderer>),
}

pub struct AsciiEngine {
    algorithm: RenderAlgorithm,
    current_cell_aspect: f32,
    inner: EngineInner,
}

impl AsciiEngine {
    pub fn new(algorithm: RenderAlgorithm, cell_aspect: f32) -> Result<Self> {
        let normalized_cell_aspect = normalize_cell_aspect(cell_aspect);
        let inner = build_inner(algorithm, normalized_cell_aspect)?;
        Ok(Self {
            algorithm,
            current_cell_aspect: normalized_cell_aspect,
            inner,
        })
    }

    pub fn algorithm(&self) -> RenderAlgorithm {
        self.algorithm
    }

    pub fn prepare_for_cell_aspect(&mut self, cell_aspect: f32) -> Result<()> {
        let normalized_cell_aspect = normalize_cell_aspect(cell_aspect);
        match &mut self.inner {
            EngineInner::LocalShape(renderer) => {
                if (self.current_cell_aspect - normalized_cell_aspect).abs() > 0.001 {
                    renderer.rebuild_glyph_bank(normalized_cell_aspect)?;
                }
            }
            EngineInner::ContextShape(renderer)
            | EngineInner::ContextShapeColor(renderer)
            | EngineInner::HalfBlockColor(renderer) => {
                let (cell_width, cell_height) = cell_dimensions_for_aspect(normalized_cell_aspect);
                if renderer.cell_width() != cell_width || renderer.cell_height() != cell_height {
                    renderer
                        .reconfigure(cell_width, cell_height)
                        .map_err(|error| anyhow!(error))?;
                }
            }
            EngineInner::Sextant(renderer)
            | EngineInner::SextantColor(renderer)
            | EngineInner::ShadeBlocks(renderer)
            | EngineInner::ShadeBlocksColor(renderer) => {
                let (cell_width, cell_height) = cell_dimensions_for_aspect(normalized_cell_aspect);
                if renderer.cell_width() != cell_width || renderer.cell_height() != cell_height {
                    renderer
                        .reconfigure(cell_width, cell_height)
                        .map_err(|error| anyhow!(error))?;
                }
            }
        }

        self.current_cell_aspect = normalized_cell_aspect;
        Ok(())
    }

    pub fn set_algorithm(&mut self, algorithm: RenderAlgorithm, cell_aspect: f32) -> Result<()> {
        let normalized_cell_aspect = normalize_cell_aspect(cell_aspect);
        if self.algorithm == algorithm {
            return self.prepare_for_cell_aspect(normalized_cell_aspect);
        }

        self.inner = build_inner(algorithm, normalized_cell_aspect)?;
        self.algorithm = algorithm;
        self.current_cell_aspect = normalized_cell_aspect;
        Ok(())
    }

    pub fn render_grayscale_ansi(
        &mut self,
        pixels: &[u8],
        width: usize,
        height: usize,
        grid: AsciiGrid,
    ) -> Result<EngineRenderedFrame> {
        match &mut self.inner {
            EngineInner::LocalShape(renderer) => {
                let frame = renderer.render_grayscale_ansi(pixels, width, height, grid)?;
                Ok(EngineRenderedFrame {
                    rows: frame.rows,
                    stats: EngineRenderStats {
                        sample_count: frame.stats.sample_count,
                        lookup_count: frame.stats.cell_count,
                        cache_hits: frame.stats.cache_hits,
                        cache_misses: frame.stats.cache_misses,
                        cell_count: frame.stats.cell_count,
                        output_bytes: frame.stats.output_bytes,
                        sgr_change_count: frame.stats.sgr_change_count,
                        timings: EngineRenderTimings {
                            total_ms: frame.stats.total_ms,
                            sample_ms: None,
                            lookup_ms: None,
                            assemble_ms: frame.stats.assemble_ms,
                        },
                    },
                })
            }
            EngineInner::ContextShape(renderer) => {
                renderer
                    .render_luma(pixels, width, height, grid.columns, grid.rows)
                    .map_err(|error| anyhow!(error))?;
                let stats = renderer.stats();
                Ok(EngineRenderedFrame {
                    rows: split_output_lines(&renderer.output_text()),
                    stats: engine_stats_from_context_shape(grid, &stats),
                })
            }
            EngineInner::Sextant(renderer) => {
                renderer
                    .render_sextant_luma(pixels, width, height, grid.columns, grid.rows)
                    .map_err(|error| anyhow!(error))?;
                let stats = renderer.stats();
                Ok(EngineRenderedFrame {
                    rows: split_output_lines(&renderer.output_text()),
                    stats: engine_stats_from_unicode_blocks(grid, &stats),
                })
            }
            EngineInner::ShadeBlocks(renderer) => {
                renderer
                    .render_shade_blocks_luma(pixels, width, height, grid.columns, grid.rows)
                    .map_err(|error| anyhow!(error))?;
                let stats = renderer.stats();
                Ok(EngineRenderedFrame {
                    rows: split_output_lines(&renderer.output_text()),
                    stats: engine_stats_from_unicode_blocks(grid, &stats),
                })
            }
            EngineInner::ContextShapeColor(_)
            | EngineInner::HalfBlockColor(_)
            | EngineInner::SextantColor(_)
            | EngineInner::ShadeBlocksColor(_) => {
                bail!("color algorithms expect RGB24; use render_rgb_ansi instead");
            }
        }
    }

    pub fn render_rgb_ansi(
        &mut self,
        rgb: &[u8],
        width: usize,
        height: usize,
        grid: AsciiGrid,
    ) -> Result<EngineRenderedFrame> {
        match &mut self.inner {
            EngineInner::ContextShapeColor(renderer) => {
                renderer
                    .render_rgb(rgb, width, height, grid.columns, grid.rows)
                    .map_err(|error| anyhow!(error))?;
                let stats = renderer.stats();
                Ok(EngineRenderedFrame {
                    rows: split_output_lines(&renderer.output_text()),
                    stats: engine_stats_from_context_shape(grid, &stats),
                })
            }
            EngineInner::HalfBlockColor(renderer) => {
                renderer
                    .render_rgb_half_blocks(rgb, width, height, grid.columns, grid.rows)
                    .map_err(|error| anyhow!(error))?;
                let stats = renderer.stats();
                Ok(EngineRenderedFrame {
                    rows: split_output_lines(&renderer.output_text()),
                    stats: engine_stats_from_context_shape(grid, &stats),
                })
            }
            EngineInner::SextantColor(renderer) => {
                renderer
                    .render_sextant_rgb(rgb, width, height, grid.columns, grid.rows)
                    .map_err(|error| anyhow!(error))?;
                let stats = renderer.stats();
                Ok(EngineRenderedFrame {
                    rows: split_output_lines(&renderer.output_text()),
                    stats: engine_stats_from_unicode_blocks(grid, &stats),
                })
            }
            EngineInner::ShadeBlocksColor(renderer) => {
                renderer
                    .render_shade_blocks_rgb(rgb, width, height, grid.columns, grid.rows)
                    .map_err(|error| anyhow!(error))?;
                let stats = renderer.stats();
                Ok(EngineRenderedFrame {
                    rows: split_output_lines(&renderer.output_text()),
                    stats: engine_stats_from_unicode_blocks(grid, &stats),
                })
            }
            _ => bail!("render_rgb_ansi requires a color algorithm"),
        }
    }
}

fn engine_stats_from_context_shape(
    grid: AsciiGrid,
    stats: &crate::context_shape::ContextShapeRenderStats,
) -> EngineRenderStats {
    EngineRenderStats {
        sample_count: stats.sample_count as usize,
        lookup_count: stats.lookup_count as usize,
        cache_hits: stats.cache_hits as usize,
        cache_misses: stats.cache_misses as usize,
        cell_count: grid.columns.saturating_mul(grid.rows),
        output_bytes: stats.output_bytes as usize,
        sgr_change_count: Some(stats.sgr_change_count as usize),
        timings: EngineRenderTimings {
            total_ms: stats.total_ms,
            sample_ms: stats.sample_ms,
            lookup_ms: stats.lookup_ms,
            assemble_ms: stats.assemble_ms,
        },
    }
}

fn engine_stats_from_unicode_blocks(
    grid: AsciiGrid,
    stats: &UnicodeBlocksRenderStats,
) -> EngineRenderStats {
    EngineRenderStats {
        sample_count: stats.sample_count as usize,
        lookup_count: stats.lookup_count as usize,
        cache_hits: stats.cache_hits as usize,
        cache_misses: stats.cache_misses as usize,
        cell_count: grid.columns.saturating_mul(grid.rows),
        output_bytes: stats.output_bytes as usize,
        sgr_change_count: Some(stats.sgr_change_count as usize),
        timings: EngineRenderTimings {
            total_ms: stats.total_ms,
            sample_ms: stats.sample_ms,
            lookup_ms: stats.lookup_ms,
            assemble_ms: stats.assemble_ms,
        },
    }
}

fn build_inner(algorithm: RenderAlgorithm, cell_aspect: f32) -> Result<EngineInner> {
    Ok(match algorithm {
        RenderAlgorithm::LocalShape => {
            let mut renderer = AsciiRenderer::new()?;
            renderer.rebuild_glyph_bank(cell_aspect)?;
            EngineInner::LocalShape(renderer)
        }
        RenderAlgorithm::ContextShape
        | RenderAlgorithm::ContextShapeColor
        | RenderAlgorithm::HalfBlockColor => {
            let (cell_width, cell_height) = cell_dimensions_for_aspect(cell_aspect);
            let renderer = ContextShapeRenderer::new(cell_width, cell_height)
                .map_err(|error| anyhow!(error))?;
            match algorithm {
                RenderAlgorithm::ContextShape => EngineInner::ContextShape(Box::new(renderer)),
                RenderAlgorithm::ContextShapeColor => {
                    EngineInner::ContextShapeColor(Box::new(renderer))
                }
                RenderAlgorithm::HalfBlockColor => EngineInner::HalfBlockColor(Box::new(renderer)),
                RenderAlgorithm::LocalShape => unreachable!("handled above"),
                RenderAlgorithm::Sextant
                | RenderAlgorithm::SextantColor
                | RenderAlgorithm::ShadeBlocks
                | RenderAlgorithm::ShadeBlocksColor => unreachable!("handled below"),
            }
        }
        RenderAlgorithm::Sextant
        | RenderAlgorithm::SextantColor
        | RenderAlgorithm::ShadeBlocks
        | RenderAlgorithm::ShadeBlocksColor => {
            let (cell_width, cell_height) = cell_dimensions_for_aspect(cell_aspect);
            let renderer = UnicodeBlocksRenderer::new(cell_width, cell_height)
                .map_err(|error| anyhow!(error))?;
            match algorithm {
                RenderAlgorithm::Sextant => EngineInner::Sextant(Box::new(renderer)),
                RenderAlgorithm::SextantColor => EngineInner::SextantColor(Box::new(renderer)),
                RenderAlgorithm::ShadeBlocks => EngineInner::ShadeBlocks(Box::new(renderer)),
                RenderAlgorithm::ShadeBlocksColor => {
                    EngineInner::ShadeBlocksColor(Box::new(renderer))
                }
                RenderAlgorithm::LocalShape
                | RenderAlgorithm::ContextShape
                | RenderAlgorithm::ContextShapeColor
                | RenderAlgorithm::HalfBlockColor => unreachable!("handled above"),
            }
        }
    })
}

fn normalize_cell_aspect(cell_aspect: f32) -> f32 {
    if cell_aspect.is_finite() && cell_aspect > 0.0 {
        cell_aspect
    } else {
        DEFAULT_CELL_ASPECT
    }
}

fn split_output_lines(output: &str) -> Vec<String> {
    if output.is_empty() {
        Vec::new()
    } else {
        output.split('\n').map(str::to_owned).collect()
    }
}

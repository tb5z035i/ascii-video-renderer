pub mod ascii;
pub mod context_shape;
pub mod engine;
mod unicode_blocks;

#[cfg(not(target_arch = "wasm32"))]
mod player;
#[cfg(not(target_arch = "wasm32"))]
mod terminal;
#[cfg(not(target_arch = "wasm32"))]
mod video;
#[cfg(target_arch = "wasm32")]
mod wasm_exports;

pub use engine::{AsciiEngine, RenderAlgorithm, RenderPixelFormat, RenderRasterDimensions};
#[cfg(not(target_arch = "wasm32"))]
pub use player::{Player, PlayerOptions};

pub mod ascii;
pub mod engine;
pub mod harri;

#[cfg(not(target_arch = "wasm32"))]
mod player;
#[cfg(not(target_arch = "wasm32"))]
mod terminal;
#[cfg(not(target_arch = "wasm32"))]
mod video;
#[cfg(target_arch = "wasm32")]
mod wasm_exports;

#[cfg(not(target_arch = "wasm32"))]
pub use player::{Player, PlayerOptions};

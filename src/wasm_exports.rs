use std::cell::RefCell;
use std::collections::HashMap;
use std::slice;

use crate::context_shape::{ContextShapeRenderStats, ContextShapeRenderer};
use crate::unicode_blocks::{UnicodeBlocksRenderStats, UnicodeBlocksRenderer};

thread_local! {
    static NEXT_HANDLE: RefCell<u32> = const { RefCell::new(1) };
    static RENDERERS: RefCell<HashMap<u32, StoredRenderer>> = RefCell::new(HashMap::new());
    static LAST_ERROR: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

enum StoredRenderer {
    ContextShape(ContextShapeRenderer),
    UnicodeBlocks(UnicodeBlocksRenderer),
}

#[derive(Clone, Copy, Debug, Default)]
struct StoredRendererStats {
    total_ms: f64,
    sample_ms: Option<f64>,
    lookup_ms: Option<f64>,
    assemble_ms: Option<f64>,
    sgr_change_count: u32,
    cache_hits: u32,
    cache_misses: u32,
    sample_count: u32,
    lookup_count: u32,
}

impl StoredRenderer {
    fn output_bytes(&self) -> &[u8] {
        match self {
            Self::ContextShape(renderer) => renderer.output_bytes(),
            Self::UnicodeBlocks(renderer) => renderer.output_bytes(),
        }
    }

    fn stats(&self) -> StoredRendererStats {
        match self {
            Self::ContextShape(renderer) => renderer.stats().into(),
            Self::UnicodeBlocks(renderer) => renderer.stats().into(),
        }
    }
}

impl From<ContextShapeRenderStats> for StoredRendererStats {
    fn from(stats: ContextShapeRenderStats) -> Self {
        Self {
            total_ms: stats.total_ms,
            sample_ms: stats.sample_ms,
            lookup_ms: stats.lookup_ms,
            assemble_ms: stats.assemble_ms,
            sgr_change_count: stats.sgr_change_count,
            cache_hits: stats.cache_hits,
            cache_misses: stats.cache_misses,
            sample_count: stats.sample_count,
            lookup_count: stats.lookup_count,
        }
    }
}

impl From<UnicodeBlocksRenderStats> for StoredRendererStats {
    fn from(stats: UnicodeBlocksRenderStats) -> Self {
        Self {
            total_ms: stats.total_ms,
            sample_ms: stats.sample_ms,
            lookup_ms: stats.lookup_ms,
            assemble_ms: stats.assemble_ms,
            sgr_change_count: stats.sgr_change_count,
            cache_hits: stats.cache_hits,
            cache_misses: stats.cache_misses,
            sample_count: stats.sample_count,
            lookup_count: stats.lookup_count,
        }
    }
}

#[no_mangle]
pub extern "C" fn alloc(len: u32) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }

    let mut buffer = Vec::<u8>::with_capacity(len as usize);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// # Safety
///
/// `ptr`, `len`, and `cap` must describe a buffer previously returned by
/// `alloc()` and not yet released.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: u32, cap: u32) {
    if ptr.is_null() || cap == 0 {
        return;
    }

    let _ = Vec::from_raw_parts(ptr, len as usize, cap as usize);
}

#[no_mangle]
pub extern "C" fn renderer_create(cell_width: u32, cell_height: u32) -> u32 {
    match ContextShapeRenderer::new(cell_width as usize, cell_height as usize) {
        Ok(renderer) => {
            let handle = next_handle();
            RENDERERS.with(|renderers| {
                renderers
                    .borrow_mut()
                    .insert(handle, StoredRenderer::ContextShape(renderer));
            });
            clear_last_error();
            handle
        }
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_create_unicode_blocks(cell_width: u32, cell_height: u32) -> u32 {
    match UnicodeBlocksRenderer::new(cell_width as usize, cell_height as usize) {
        Ok(renderer) => {
            let handle = next_handle();
            RENDERERS.with(|renderers| {
                renderers
                    .borrow_mut()
                    .insert(handle, StoredRenderer::UnicodeBlocks(renderer));
            });
            clear_last_error();
            handle
        }
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_destroy(handle: u32) {
    RENDERERS.with(|renderers| {
        renderers.borrow_mut().remove(&handle);
    });
}

/// # Safety
///
/// When `pixels_len` is non-zero, `pixels_ptr` must point to a readable buffer
/// of `pixels_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn renderer_render(
    handle: u32,
    pixels_ptr: *const u8,
    pixels_len: u32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
) -> u32 {
    let pixels = if pixels_len == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(pixels_ptr, pixels_len as usize)
    };

    match with_context_renderer_mut(handle, |renderer| {
        renderer.render_luma(
            pixels,
            width as usize,
            height as usize,
            columns as usize,
            rows as usize,
        )
    }) {
        Ok(()) => {
            clear_last_error();
            1
        }
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

/// # Safety
///
/// When `pixels_len` is non-zero, `pixels_ptr` must point to a readable buffer
/// of `pixels_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn renderer_render_half_block_rgb(
    handle: u32,
    pixels_ptr: *const u8,
    pixels_len: u32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
) -> u32 {
    let pixels = if pixels_len == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(pixels_ptr, pixels_len as usize)
    };

    match with_context_renderer_mut(handle, |renderer| {
        renderer.render_rgb_half_blocks(
            pixels,
            width as usize,
            height as usize,
            columns as usize,
            rows as usize,
        )
    }) {
        Ok(()) => {
            clear_last_error();
            1
        }
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

/// # Safety
///
/// When `pixels_len` is non-zero, `pixels_ptr` must point to a readable buffer
/// of `pixels_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn renderer_render_sextant(
    handle: u32,
    pixels_ptr: *const u8,
    pixels_len: u32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
) -> u32 {
    let pixels = if pixels_len == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(pixels_ptr, pixels_len as usize)
    };

    match with_unicode_renderer_mut(handle, |renderer| {
        renderer.render_sextant_luma(
            pixels,
            width as usize,
            height as usize,
            columns as usize,
            rows as usize,
        )
    }) {
        Ok(()) => {
            clear_last_error();
            1
        }
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

/// # Safety
///
/// When `pixels_len` is non-zero, `pixels_ptr` must point to a readable buffer
/// of `pixels_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn renderer_render_sextant_rgb(
    handle: u32,
    pixels_ptr: *const u8,
    pixels_len: u32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
) -> u32 {
    let pixels = if pixels_len == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(pixels_ptr, pixels_len as usize)
    };

    match with_unicode_renderer_mut(handle, |renderer| {
        renderer.render_sextant_rgb(
            pixels,
            width as usize,
            height as usize,
            columns as usize,
            rows as usize,
        )
    }) {
        Ok(()) => {
            clear_last_error();
            1
        }
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

/// # Safety
///
/// When `pixels_len` is non-zero, `pixels_ptr` must point to a readable buffer
/// of `pixels_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn renderer_render_shade_blocks(
    handle: u32,
    pixels_ptr: *const u8,
    pixels_len: u32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
) -> u32 {
    let pixels = if pixels_len == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(pixels_ptr, pixels_len as usize)
    };

    match with_unicode_renderer_mut(handle, |renderer| {
        renderer.render_shade_blocks_luma(
            pixels,
            width as usize,
            height as usize,
            columns as usize,
            rows as usize,
        )
    }) {
        Ok(()) => {
            clear_last_error();
            1
        }
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

/// # Safety
///
/// When `pixels_len` is non-zero, `pixels_ptr` must point to a readable buffer
/// of `pixels_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn renderer_render_shade_blocks_rgb(
    handle: u32,
    pixels_ptr: *const u8,
    pixels_len: u32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
) -> u32 {
    let pixels = if pixels_len == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(pixels_ptr, pixels_len as usize)
    };

    match with_unicode_renderer_mut(handle, |renderer| {
        renderer.render_shade_blocks_rgb(
            pixels,
            width as usize,
            height as usize,
            columns as usize,
            rows as usize,
        )
    }) {
        Ok(()) => {
            clear_last_error();
            1
        }
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_output_ptr(handle: u32) -> u32 {
    match with_renderer(handle, |renderer| renderer.output_bytes().as_ptr() as usize) {
        Ok(ptr) => ptr as u32,
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_output_len(handle: u32) -> u32 {
    match with_renderer(handle, |renderer| renderer.output_bytes().len()) {
        Ok(len) => saturating_u32(len),
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_sgr_change_count(handle: u32) -> u32 {
    match with_renderer(handle, |renderer| renderer.stats().sgr_change_count) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_cache_hits(handle: u32) -> u32 {
    match with_renderer(handle, |renderer| renderer.stats().cache_hits) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_cache_misses(handle: u32) -> u32 {
    match with_renderer(handle, |renderer| renderer.stats().cache_misses) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_sample_count(handle: u32) -> u32 {
    match with_renderer(handle, |renderer| renderer.stats().sample_count) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_lookup_count(handle: u32) -> u32 {
    match with_renderer(handle, |renderer| renderer.stats().lookup_count) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_total_ms(handle: u32) -> f64 {
    match with_renderer(handle, |renderer| renderer.stats().total_ms) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            f64::NAN
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_sample_ms(handle: u32) -> f64 {
    match with_renderer(handle, |renderer| {
        renderer.stats().sample_ms.unwrap_or(f64::NAN)
    }) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            f64::NAN
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_lookup_ms(handle: u32) -> f64 {
    match with_renderer(handle, |renderer| {
        renderer.stats().lookup_ms.unwrap_or(f64::NAN)
    }) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            f64::NAN
        }
    }
}

#[no_mangle]
pub extern "C" fn renderer_assemble_ms(handle: u32) -> f64 {
    match with_renderer(handle, |renderer| {
        renderer.stats().assemble_ms.unwrap_or(f64::NAN)
    }) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            f64::NAN
        }
    }
}

#[no_mangle]
pub extern "C" fn last_error_ptr() -> u32 {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr() as usize as u32)
}

#[no_mangle]
pub extern "C" fn last_error_len() -> u32 {
    LAST_ERROR.with(|slot| saturating_u32(slot.borrow().len()))
}

fn next_handle() -> u32 {
    NEXT_HANDLE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let handle = *slot;
        *slot = if *slot == u32::MAX { 1 } else { *slot + 1 };
        handle
    })
}

fn set_last_error(error: impl ToString) {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = error.to_string().into_bytes();
    });
}

fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        slot.borrow_mut().clear();
    });
}

fn with_renderer_mut<T>(
    handle: u32,
    callback: impl FnOnce(&mut StoredRenderer) -> Result<T, String>,
) -> Result<T, String> {
    RENDERERS.with(|renderers| {
        let mut renderers = renderers.borrow_mut();
        let renderer = renderers
            .get_mut(&handle)
            .ok_or_else(|| format!("unknown renderer handle {handle}"))?;
        callback(renderer)
    })
}

fn with_context_renderer_mut<T>(
    handle: u32,
    callback: impl FnOnce(&mut ContextShapeRenderer) -> Result<T, String>,
) -> Result<T, String> {
    with_renderer_mut(handle, |renderer| match renderer {
        StoredRenderer::ContextShape(renderer) => callback(renderer),
        StoredRenderer::UnicodeBlocks(_) => Err(format!(
            "renderer handle {handle} is not a context-shape renderer"
        )),
    })
}

fn with_unicode_renderer_mut<T>(
    handle: u32,
    callback: impl FnOnce(&mut UnicodeBlocksRenderer) -> Result<T, String>,
) -> Result<T, String> {
    with_renderer_mut(handle, |renderer| match renderer {
        StoredRenderer::UnicodeBlocks(renderer) => callback(renderer),
        StoredRenderer::ContextShape(_) => Err(format!(
            "renderer handle {handle} is not a unicode-block renderer"
        )),
    })
}

fn with_renderer<T>(handle: u32, callback: impl FnOnce(&StoredRenderer) -> T) -> Result<T, String> {
    RENDERERS.with(|renderers| {
        let renderers = renderers.borrow();
        let renderer = renderers
            .get(&handle)
            .ok_or_else(|| format!("unknown renderer handle {handle}"))?;
        Ok(callback(renderer))
    })
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

use ascii_video_renderer::ascii::AsciiGrid;
use ascii_video_renderer::engine::{AsciiEngine, RenderAlgorithm};

#[test]
fn shared_engine_renders_deterministic_grayscale_input_across_algorithms() {
    let grid = AsciiGrid {
        columns: 32,
        rows: 12,
    };
    let width = 320usize;
    let height = 192usize;
    let pixels = gradient_frame(width, height);

    let mut engine = AsciiEngine::new(RenderAlgorithm::LocalShape, 2.0)
        .expect("local-shape engine should initialize");
    let local = engine
        .render_grayscale_ansi(&pixels, width, height, grid)
        .expect("local-shape render should succeed");
    assert_eq!(local.rows.len(), grid.rows);
    assert!(local.stats.output_bytes > 0);

    engine
        .set_algorithm(RenderAlgorithm::ContextShape, 2.0)
        .expect("context-shape engine should initialize");
    let context = engine
        .render_grayscale_ansi(&pixels, width, height, grid)
        .expect("context-shape render should succeed");
    assert_eq!(context.rows.len(), grid.rows);
    assert!(context.stats.output_bytes > 0);
    assert_eq!(engine.algorithm(), RenderAlgorithm::ContextShape);

    let rgb = gradient_rgb_frame(width, height);
    engine
        .set_algorithm(RenderAlgorithm::ContextShapeColor, 2.0)
        .expect("context-shape color engine should initialize");
    let color = engine
        .render_rgb_ansi(&rgb, width, height, grid)
        .expect("context-shape color render should succeed");
    assert_eq!(color.rows.len(), grid.rows);
    assert!(color.stats.output_bytes > 0);
    assert_eq!(engine.algorithm(), RenderAlgorithm::ContextShapeColor);
}

fn gradient_frame(width: usize, height: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; width.saturating_mul(height)];
    for y in 0..height {
        for x in 0..width {
            let horizontal = x as f32 / width.max(1) as f32;
            let vertical = y as f32 / height.max(1) as f32;
            pixels[y * width + x] = ((horizontal * 0.7 + vertical * 0.3) * 255.0).round() as u8;
        }
    }
    pixels
}

fn gradient_rgb_frame(width: usize, height: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; width.saturating_mul(height).saturating_mul(3)];
    for y in 0..height {
        for x in 0..width {
            let horizontal = x as f32 / width.max(1) as f32;
            let vertical = y as f32 / height.max(1) as f32;
            let i = (y * width + x) * 3;
            pixels[i] = (horizontal * 200.0) as u8;
            pixels[i + 1] = (vertical * 220.0) as u8;
            pixels[i + 2] = ((horizontal + vertical) * 0.5 * 255.0) as u8;
        }
    }
    pixels
}

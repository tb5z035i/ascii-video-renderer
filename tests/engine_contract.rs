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

    let mut engine =
        AsciiEngine::new(RenderAlgorithm::Classic, 2.0).expect("classic engine should initialize");
    let classic = engine
        .render_grayscale_ansi(&pixels, width, height, grid)
        .expect("classic render should succeed");
    assert_eq!(classic.rows.len(), grid.rows);
    assert!(classic.stats.output_bytes > 0);

    engine
        .set_algorithm(RenderAlgorithm::Harri, 2.0)
        .expect("Harri engine should initialize");
    let harri = engine
        .render_grayscale_ansi(&pixels, width, height, grid)
        .expect("Harri render should succeed");
    assert_eq!(harri.rows.len(), grid.rows);
    assert!(harri.stats.output_bytes > 0);
    assert_eq!(engine.algorithm(), RenderAlgorithm::Harri);
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

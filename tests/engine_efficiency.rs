use std::time::Instant;

use ascii_video_renderer::ascii::AsciiGrid;
use ascii_video_renderer::engine::{AsciiEngine, RenderAlgorithm};

/// Warm renders so caches and allocator settle before timing.
const WARMUP_RENDERS: usize = 3;
/// Timed renders — larger values smooth wall-clock and `getrusage` noise.
const BENCHMARK_RENDERS: usize = 1024;

#[derive(Clone, Copy, Debug)]
struct ResourceSnapshot {
    user_cpu_ms: f64,
    sys_cpu_ms: f64,
    max_rss_kib: i64,
}

#[test]
fn engine_efficiency_reports_resource_usage() {
    let grid = AsciiGrid {
        columns: 96,
        rows: 28,
    };
    let width = 640usize;
    let height = 360usize;
    let pixels = synthetic_frame(width, height);

    let rgb = synthetic_rgb_frame(width, height);

    for algorithm in [
        RenderAlgorithm::LocalShape,
        RenderAlgorithm::ContextShape,
        RenderAlgorithm::ContextShapeColor,
    ] {
        let mut engine =
            AsciiEngine::new(algorithm, 2.0).expect("engine should initialize for benchmark");

        for _ in 0..WARMUP_RENDERS {
            let frame = if algorithm.needs_rgb_frames() {
                engine
                    .render_rgb_ansi(&rgb, width, height, grid)
                    .expect("warmup rgb render should succeed")
            } else {
                engine
                    .render_grayscale_ansi(&pixels, width, height, grid)
                    .expect("warmup render should succeed")
            };
            assert_eq!(frame.rows.len(), grid.rows);
        }

        let resources_before = resource_snapshot();
        let started_at = Instant::now();
        let mut output_bytes = 0usize;
        for _ in 0..BENCHMARK_RENDERS {
            let frame = if algorithm.needs_rgb_frames() {
                engine
                    .render_rgb_ansi(&rgb, width, height, grid)
                    .expect("benchmark rgb render should succeed")
            } else {
                engine
                    .render_grayscale_ansi(&pixels, width, height, grid)
                    .expect("benchmark render should succeed")
            };
            output_bytes = frame.stats.output_bytes;
            assert_eq!(frame.rows.len(), grid.rows);
            assert!(frame.stats.timings.total_ms.is_finite());
        }
        let wall_ms = started_at.elapsed().as_secs_f64() * 1_000.0;
        let resources_after = resource_snapshot();

        let user_cpu_ms = (resources_after.user_cpu_ms - resources_before.user_cpu_ms).max(0.0);
        let sys_cpu_ms = (resources_after.sys_cpu_ms - resources_before.sys_cpu_ms).max(0.0);
        let n = BENCHMARK_RENDERS as f64;

        eprintln!(
            "engine-efficiency algorithm={} renders={} wall_ms_total={:.2} wall_ms_per_render={:.4} user_cpu_ms_total={:.2} user_cpu_ms_per_render={:.4} sys_cpu_ms_total={:.2} max_rss_kib={} output_bytes={}",
            algorithm.id(),
            BENCHMARK_RENDERS,
            wall_ms,
            wall_ms / n,
            user_cpu_ms,
            user_cpu_ms / n,
            sys_cpu_ms,
            resources_after.max_rss_kib,
            output_bytes,
        );

        assert!(output_bytes > 0, "render output should not be empty");
        assert!(wall_ms.is_finite(), "wall time should be finite");
    }
}

fn synthetic_frame(width: usize, height: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; width.saturating_mul(height)];
    for y in 0..height {
        for x in 0..width {
            let nx = x as f32 / width.max(1) as f32;
            let ny = y as f32 / height.max(1) as f32;
            let diagonal = ((nx + ny) * 127.0).round() as u8;
            let radial = {
                let dx = nx - 0.5;
                let dy = ny - 0.5;
                let distance = (dx * dx + dy * dy).sqrt();
                ((1.0 - (distance * 1.8).clamp(0.0, 1.0)) * 128.0).round() as u8
            };
            pixels[y * width + x] = diagonal.saturating_add(radial);
        }
    }
    pixels
}

fn synthetic_rgb_frame(width: usize, height: usize) -> Vec<u8> {
    let mut buf = vec![0u8; width.saturating_mul(height).saturating_mul(3)];
    for y in 0..height {
        for x in 0..width {
            let nx = x as f32 / width.max(1) as f32;
            let ny = y as f32 / height.max(1) as f32;
            let i = (y * width + x) * 3;
            buf[i] = (nx * 255.0) as u8;
            buf[i + 1] = (ny * 255.0) as u8;
            buf[i + 2] = ((nx + ny) * 0.5 * 255.0) as u8;
        }
    }
    buf
}

fn resource_snapshot() -> ResourceSnapshot {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    assert_eq!(status, 0, "getrusage should succeed");
    let usage = unsafe { usage.assume_init() };
    ResourceSnapshot {
        user_cpu_ms: timeval_ms(usage.ru_utime),
        sys_cpu_ms: timeval_ms(usage.ru_stime),
        max_rss_kib: usage.ru_maxrss,
    }
}

fn timeval_ms(value: libc::timeval) -> f64 {
    value.tv_sec as f64 * 1_000.0 + value.tv_usec as f64 / 1_000.0
}

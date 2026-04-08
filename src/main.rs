use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use anyhow::{Context, Result};
#[cfg(not(target_arch = "wasm32"))]
use ascii_video_renderer::{Player, PlayerOptions};

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn run() -> Result<()> {
    let args = parse_args()?;
    let mut player = Player::new(PlayerOptions {
        input: args.video_path,
        max_frames: args.max_frames,
    })?;
    player.run()
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct CliArgs {
    video_path: PathBuf,
    max_frames: Option<u64>,
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_args() -> Result<CliArgs> {
    let mut args = std::env::args().skip(1);
    let mut max_frames = None;
    let mut video_path = None;

    while let Some(arg) = args.next() {
        if arg == "--max-frames" {
            let value = args
                .next()
                .context("missing value for --max-frames")?
                .parse::<u64>()
                .context("--max-frames must be a positive integer")?;
            max_frames = Some(value);
            continue;
        }

        if arg == "-h" || arg == "--help" {
            print_usage();
            std::process::exit(0);
        }

        if video_path.is_some() {
            anyhow::bail!("unexpected extra positional argument: {arg}");
        }

        video_path = Some(PathBuf::from(arg));
    }

    let video_path = video_path.context("missing input mp4 path")?;
    if !video_path.is_file() {
        anyhow::bail!(
            "input path does not exist or is not a file: {}",
            video_path.display()
        );
    }

    Ok(CliArgs {
        video_path,
        max_frames,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn print_usage() {
    eprintln!(
        "Usage: ascii-video-renderer [--max-frames N] <input.mp4>\n\
         Plays a local MP4 file as resizable ASCII video in the terminal.\n\
         Controls: press `r` to cycle renderers (Local -> Context -> Color -> HalfBlk -> Sextant -> SextRGB -> Shade -> ShdRGB -> …), `Ctrl+C` to exit.\n\
         Color mode uses truecolor ANSI (24-bit fg); use a terminal that supports it."
    );
}

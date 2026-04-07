use std::path::PathBuf;

use anyhow::{Context, Result};
use ascii_video_renderer::{Player, PlayerOptions};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;
    let mut player = Player::new(PlayerOptions {
        input: args.video_path,
        max_frames: args.max_frames,
    })?;
    player.run()
}

#[derive(Debug)]
struct CliArgs {
    video_path: PathBuf,
    max_frames: Option<u64>,
}

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

fn print_usage() {
    eprintln!(
        "Usage: ascii-video-renderer [--max-frames N] <input.mp4>\n\
         Plays a local MP4 file as resizable ASCII video in the terminal."
    );
}

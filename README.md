# ascii-video-renderer

A Rust terminal executable that plays a local MP4 as live ASCII video using a
shape-matching rasterizer introduced by Alex Harri in
“ASCII characters are not pixels” article.

## Features

- local MP4 playback in the terminal
- 6-sample shape-vector ASCII rasterization
- dynamic resize support using current terminal dimensions
- terminal cell aspect ratio from `TIOCGWINSZ`
- fallback cell aspect ratio of `2:1` when pixel metrics are unavailable
- live status bar with measured FPS and decode-to-render latency
- bounded-memory latest-frame pipeline to avoid unbounded buffering

## Requirements

- Rust toolchain
- `ffmpeg`
- `ffprobe`
- a readable monospace font discoverable through `fc-match`
  - fallback path: `/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf`

## Build

```bash
cargo build
```

## Run

```bash
cargo run -- /path/to/video.mp4
```

### Test / smoke mode

To exit automatically after a few rendered frames:

```bash
cargo run -- --max-frames 10 /path/to/video.mp4
```

This is mainly intended for automated smoke tests.

## Terminal sizing behavior

- terminal rows and columns are taken from `TIOCGWINSZ`
- terminal pixel metrics are also read from `TIOCGWINSZ`
- if pixel metrics are present, the player derives the terminal cell
  height-to-width ratio from them
- if pixel metrics are zero or unavailable, the player falls back to a `2:1`
  cell ratio as requested
- the last terminal row is reserved for the status bar

## Controls

- `Ctrl+C` to stop playback

## Acknowledgements

Special thanks to [alexharri](https://github.com/alexharri). The rendering
mechanism used here is based on the approach introduced in
[ASCII characters are not pixels](https://alexharri.com/blog/ascii-rendering).

## Test commands

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Example synthetic video for testing:

```bash
ffmpeg -y -f lavfi -i "testsrc2=size=640x480:rate=60" -t 1 /tmp/testsrc.mp4
cargo run -- --max-frames 5 /tmp/testsrc.mp4
```

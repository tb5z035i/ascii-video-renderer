use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn player_runs_in_a_pty_for_a_few_frames() {
    let workspace = workspace_root();
    let video_path = workspace.join("target").join("smoke-testsrc.mp4");

    let ffmpeg_status = Command::new("ffmpeg")
        .current_dir(&workspace)
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x480:rate=60",
            "-t",
            "1",
        ])
        .arg(&video_path)
        .status()
        .expect("ffmpeg should be available for smoke test");
    assert!(
        ffmpeg_status.success(),
        "ffmpeg failed to generate smoke video"
    );

    let binary = workspace
        .join("target")
        .join("debug")
        .join("ascii-video-renderer");
    let capture = workspace.join("target").join("smoke-script-output.txt");

    let command = format!(
        "stty rows 40 cols 120; \"{}\" --max-frames 3 \"{}\"",
        binary.display(),
        video_path.display()
    );

    let status = Command::new("script")
        .current_dir(&workspace)
        .args(["-qec", &command])
        .arg(&capture)
        .status()
        .expect("script should be available for PTY smoke test");

    assert!(status.success(), "player should exit successfully in PTY");
}

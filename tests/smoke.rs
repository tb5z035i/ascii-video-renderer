use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn generate_smoke_video(workspace: &PathBuf, name: &str, duration_secs: &str) -> PathBuf {
    let video_path = workspace.join("target").join(name);

    let ffmpeg_status = Command::new("ffmpeg")
        .current_dir(workspace)
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x480:rate=60",
            "-t",
            duration_secs,
        ])
        .arg(&video_path)
        .status()
        .expect("ffmpeg should be available for smoke test");
    assert!(
        ffmpeg_status.success(),
        "ffmpeg failed to generate smoke video"
    );

    video_path
}

#[test]
fn player_runs_in_a_pty_for_a_few_frames() {
    let workspace = workspace_root();
    let video_path = generate_smoke_video(&workspace, "smoke-testsrc.mp4", "1");

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

#[test]
fn player_exits_when_ctrl_c_is_sent_in_raw_mode() {
    let workspace = workspace_root();
    let video_path = generate_smoke_video(&workspace, "smoke-ctrl-c.mp4", "5");

    let binary = workspace
        .join("target")
        .join("debug")
        .join("ascii-video-renderer");
    let capture = workspace.join("target").join("smoke-ctrl-c-output.txt");

    let command = format!(
        "stty rows 40 cols 120; \"{}\" \"{}\" < /dev/tty & pid=$!; sleep 0.25; printf '\\003' > /dev/tty; wait \"$pid\"",
        binary.display(),
        video_path.display()
    );

    let status = Command::new("script")
        .current_dir(&workspace)
        .args(["-qec", &command])
        .arg(&capture)
        .status()
        .expect("script should be available for PTY smoke test");

    assert!(status.success(), "player should exit cleanly after Ctrl+C");
}

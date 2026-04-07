use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve")
}

/// Path to the `ascii-video-renderer` binary for the same build profile as this test crate.
/// Uses `CARGO_BIN_EXE_*` when present (correct for `cargo test` / `cargo test --release`);
/// otherwise falls back to `target/(debug|release)/` next to this manifest.
fn ascii_player_binary() -> PathBuf {
    for key in [
        "CARGO_BIN_EXE_ascii_video_renderer",
        "CARGO_BIN_EXE_ascii-video-renderer",
    ] {
        if let Some(path) = std::env::var_os(key) {
            return PathBuf::from(path);
        }
    }
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(profile)
        .join("ascii-video-renderer")
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

    let binary = ascii_player_binary();
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

    let binary = ascii_player_binary();
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

#[test]
fn player_toggles_render_mode_with_r_key() {
    let workspace = workspace_root();
    let video_path = generate_smoke_video(&workspace, "smoke-toggle-mode.mp4", "5");

    let binary = ascii_player_binary();
    let capture = workspace
        .join("target")
        .join("smoke-toggle-mode-output.txt");

    let driver_script = r#"
import os, pty, select, subprocess, sys, time, termios, fcntl, struct, pathlib

binary, video, workspace, capture = sys.argv[1:5]
workspace = pathlib.Path(workspace)
capture = pathlib.Path(capture)
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
proc = subprocess.Popen(
    [binary, "--max-frames", "120", video],
    stdin=slave,
    stdout=slave,
    stderr=slave,
    cwd=workspace,
)
os.close(slave)
chunks = []
start = time.time()
sent_r = False
sent_ctrl = False
deadline = start + 4.0

while time.time() < deadline and proc.poll() is None:
    now = time.time()
    if not sent_r and now - start >= 0.5:
        os.write(master, b"r")
        sent_r = True
    if sent_r and not sent_ctrl and now - start >= 1.0:
        os.write(master, b"\x03")
        sent_ctrl = True
    rlist, _, _ = select.select([master], [], [], 0.1)
    if master in rlist:
        try:
            chunks.append(os.read(master, 65536))
        except OSError:
            break

if proc.poll() is None:
    proc.terminate()
    proc.wait(timeout=1)

while True:
    try:
        data = os.read(master, 65536)
        if not data:
            break
        chunks.append(data)
    except OSError:
        break

os.close(master)
capture.write_bytes(b"".join(chunks))
sys.exit(proc.returncode or 0)
"#;

    let status = Command::new("python3")
        .current_dir(&workspace)
        .arg("-c")
        .arg(driver_script)
        .arg(&binary)
        .arg(&video_path)
        .arg(&workspace)
        .arg(&capture)
        .status()
        .expect("python3 should be available for PTY smoke test");

    assert!(
        status.success(),
        "player should survive renderer toggle input"
    );
    let capture_text = fs::read_to_string(&capture).expect("capture output should exist");
    assert!(
        capture_text.contains("mode Context"),
        "status line should report the toggled renderer mode"
    );
}

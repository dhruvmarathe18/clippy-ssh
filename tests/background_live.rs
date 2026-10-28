// Opt-in desktop test. Uses copied binaries and a private per-directory pipe,
// so it never stops or changes the user's installed background process.
#![cfg(windows)]
use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct App {
    folder: tempfile::TempDir,
    client: PathBuf,
}
impl App {
    fn command(&self, action: &str) -> std::process::Output {
        let (done, wait) = mpsc::channel();
        let client = self.client.clone();
        let watchdog = thread::spawn(move || {
            if wait.recv_timeout(Duration::from_secs(15)).is_err() {
                // Only this test's copied installation is stopped if output handles leak.
                let _ = Command::new(client)
                    .args(["background", "stop"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        });
        let start = Instant::now();
        let output = Command::new(&self.client)
            .args(["background", action])
            .output()
            .unwrap();
        let _ = done.send(());
        watchdog.join().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "background {action} retained output handles or stalled"
        );
        output
    }
}
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.command("stop");
        for _ in 0..40 {
            if !self.command("status").status.success() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        // Both files are under this newly-created, canonical temporary directory.
        assert!(
            self.client
                .canonicalize()
                .unwrap()
                .starts_with(self.folder.path().canonicalize().unwrap())
        );
    }
}
#[test]
fn background_lifecycle_and_peer_authentication() {
    if std::env::var_os("CLIPPY_TEST_BACKGROUND").is_none() {
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let client = folder.path().join("clippy.exe");
    fs::copy(env!("CARGO_BIN_EXE_clippy"), &client).unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_clippy-background"),
        folder.path().join("clippy-background.exe"),
    )
    .unwrap();
    let app = App { folder, client };
    for action in ["start", "check", "pause", "status", "resume", "check"] {
        eprintln!("Checking background {action}");
        let output = app.command(action);
        assert!(
            output.status.success(),
            "{action}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if action == "pause" || action == "status" {
            assert!(String::from_utf8_lossy(&output.stdout).contains("paused"));
        }
    }
    let endpoint = clippy_ssh::windows::ipc::endpoint_for(&app.client).unwrap();
    eprintln!("Checking unauthorized peer rejection");
    let mut unauthorized = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(endpoint)
        .unwrap();
    let mut byte = [0];
    // The test process has the same logon SID, but is not the registered client binary.
    assert!(matches!(unauthorized.read(&mut byte), Ok(0) | Err(_)));
    drop(unauthorized);
    let status = app.command("status");
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("0 session(s)"));
}

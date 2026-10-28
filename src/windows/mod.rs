pub mod automatic;
pub mod background;
pub mod bridge;
pub mod client;
pub mod clipboard;
pub mod console;
pub mod install;
pub mod ipc;

pub fn doctor() -> anyhow::Result<()> {
    println!("Clippy {}", env!("CARGO_PKG_VERSION"));
    println!(
        "Windows Terminal session: {}",
        std::env::var("WT_SESSION")
            .unwrap_or_else(|_| "not detected (run from Windows Terminal)".into())
    );
    println!(
        "OpenSSH: {}",
        std::process::Command::new("ssh.exe")
            .arg("-V")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string())
            .unwrap_or_else(|e| e.to_string())
    );
    println!(
        "Clipboard files/image available: {}",
        clipboard::has_rich_content()
    );
    println!("Helper version required: {}", env!("CARGO_PKG_VERSION"));
    background::check_hook_support()?;
    println!("Windows keyboard-hook support: available");
    match background::control(ipc::Action::Status) {
        Ok(ipc::Response::Status {
            pid,
            paused,
            sessions,
        }) => println!("Background: PID {pid}, paused={paused}, sessions={sessions}"),
        _ => println!("Background: not running for this installation"),
    }
    println!("Run clippy install once, then use ssh in a new PowerShell tab in Windows Terminal.");
    Ok(())
}

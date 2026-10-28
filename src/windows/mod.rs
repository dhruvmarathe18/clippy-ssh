pub mod bridge;
pub mod client;
pub mod clipboard;
pub mod console;

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
    bridge::check_hook_support()?;
    println!("Windows keyboard-hook support: available");
    println!("Run clippy setup [SSH OPTIONS] HOST once, then clippy ssh [SSH OPTIONS] HOST.");
    Ok(())
}

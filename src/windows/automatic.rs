use crate::{
    integration::automatic_config_supported,
    ssh::{self, Connection},
};
use anyhow::Result;
use std::{
    io::IsTerminal,
    process::{Command, Stdio},
};

fn native(args: &[String]) -> Result<()> {
    let status = Command::new("ssh.exe").args(args).status()?;
    std::process::exit(status.code().unwrap_or(255));
}
pub fn run(args: &[String]) -> Result<()> {
    // Never intercept automation, pipelines, non-Windows-Terminal hosts, or opt-outs.
    if !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || std::env::var_os("WT_SESSION").is_none()
        || std::env::var("TERM_PROGRAM").as_deref() == Ok("vscode")
        || std::env::var_os("CLIPPY_DISABLE").is_some()
    {
        return native(args);
    }
    let Ok(connection) = Connection::parse(args) else {
        return native(args);
    };
    let config = Command::new("ssh.exe")
        .arg("-G")
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    if !config.status.success()
        || !automatic_config_supported(&String::from_utf8_lossy(&config.stdout))
    {
        return native(args);
    }
    let probe = connection.command(&format!(
        "if [ \"$(uname -s 2>/dev/null)\" != Linux ]; then exit 42; fi; test -x \"$HOME/.local/lib/clippy/{}/clippy-agent\"",
        env!("CARGO_PKG_VERSION")
    )).stdin(Stdio::inherit()).stdout(Stdio::null()).stderr(Stdio::inherit()).status()?;
    match probe.code() {
        Some(0) => {}
        Some(1) => {
            eprintln!("Clippy: installing the Linux helper for this version...");
            if let Err(error) = ssh::setup(&connection, None) {
                eprintln!("Clippy setup unavailable ({error}); connecting with native SSH.");
                return native(args);
            }
        }
        Some(255) | None => std::process::exit(probe.code().unwrap_or(255)),
        _ => return native(args),
    }
    super::client::connect(connection)
}

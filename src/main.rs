use anyhow::{Result, bail};
use clippy_ssh::ssh::{self, Connection};
fn main() {
    if let Err(e) = run() {
        eprintln!("clippy: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        println!(
            "Clippy {} — clipboard files and images over SSH\n\n  clippy setup [--agent PATH] [SSH OPTIONS] HOST\n  clippy ssh [SSH OPTIONS] HOST\n  clippy clean --older-than 30d [SSH OPTIONS] HOST\n  clippy doctor\n  clippy last\n\nPaste text normally. Ctrl+V / Ctrl+Shift+V upload files or images and insert remote paths.\nNo success popup or automatic Enter. Supports Linux and tmux; jump hosts use -J or SSH configuration.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    match args.remove(0).as_str() {
        "--version" => println!("clippy {}", env!("CARGO_PKG_VERSION")),
        "setup" => {
            let path = take_option(&mut args, "--agent")?;
            ssh::setup(
                &Connection::parse(&args)?,
                path.as_deref().map(std::path::Path::new),
            )?;
        }
        "clean" => {
            let age = take_option(&mut args, "--older-than")?
                .ok_or_else(|| anyhow::anyhow!("clean requires --older-than, e.g. 30d"))?;
            ssh::cleanup(&Connection::parse(&args)?, &age)?;
        }
        #[cfg(windows)]
        "ssh" => clippy_ssh::windows::client::connect(Connection::parse(&args)?)?,
        #[cfg(windows)]
        "doctor" => clippy_ssh::windows::doctor()?,
        #[cfg(windows)]
        "last" => clippy_ssh::windows::client::last()?,
        #[cfg(not(windows))]
        "ssh" | "doctor" => {
            bail!("the clipboard client currently supports Windows; clippy-agent supports Linux")
        }
        other => bail!("unknown command {other}; run clippy --help"),
    }
    Ok(())
}
fn take_option(args: &mut Vec<String>, name: &str) -> Result<Option<String>> {
    if let Some(i) = args.iter().position(|a| a == name) {
        args.remove(i);
        if i >= args.len() {
            bail!("{name} needs a value");
        }
        Ok(Some(args.remove(i)))
    } else {
        Ok(None)
    }
}

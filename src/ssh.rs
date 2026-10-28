use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug, Clone)]
pub struct Connection {
    pub options: Vec<String>,
    pub host: String,
}
impl Connection {
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut options = vec![];
        let mut host = None;
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if arg == "--" {
                i += 1;
                ensure!(i + 1 == args.len(), "provide one SSH destination");
                host = Some(args[i].clone());
                break;
            }
            if arg.starts_with('-') {
                ensure!(host.is_none(), "place SSH options before HOST");
                if ["-4", "-6", "-A", "-a", "-C", "-v", "-vv", "-vvv"].contains(&arg.as_str()) {
                    options.push(arg.clone());
                } else {
                    let prefix = arg.get(..2).unwrap_or("");
                    ensure!(
                        ["-J", "-i", "-p", "-F", "-l", "-o"].contains(&prefix),
                        "unsupported SSH option {arg}; Clippy owns the remote command and PTY"
                    );
                    let value = if arg.len() > 2 {
                        arg[2..].to_string()
                    } else {
                        i += 1;
                        args.get(i).context("SSH option needs a value")?.clone()
                    };
                    if prefix == "-o" {
                        let key = value
                            .split(['=', ' '])
                            .next()
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        ensure!(
                            ![
                                "remotecommand",
                                "requesttty",
                                "sessiontype",
                                "stdinnull",
                                "forkafterauthentication",
                                "localcommand",
                                "permitlocalcommand"
                            ]
                            .contains(&key.as_str()),
                            "SSH option {key} conflicts with the Clippy transport"
                        );
                    }
                    options.push(prefix.into());
                    options.push(value);
                }
            } else {
                ensure!(
                    host.is_none(),
                    "remote commands are not supported; run them inside the session"
                );
                host = Some(arg.clone());
            }
            i += 1;
        }
        let host = host.context("missing SSH destination")?;
        ensure!(
            !host.is_empty() && !host.starts_with('-') && !host.chars().any(char::is_control),
            "invalid destination"
        );
        Ok(Self { options, host })
    }
    pub fn command(&self, script: &str) -> Command {
        let mut command = Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" });
        command
            .args(["-T", "-o", "RemoteCommand=none", "-o", "RequestTTY=no"])
            .args(&self.options)
            .arg("--")
            .arg(&self.host)
            .arg(script);
        command
    }
    pub fn serve(&self) -> Command {
        self.command(&format!(
            "exec \"$HOME/.local/lib/clippy/{}/clippy-agent\" serve",
            env!("CARGO_PKG_VERSION")
        ))
    }
}
pub fn setup(connection: &Connection, agent_override: Option<&Path>) -> Result<()> {
    let output = connection
        .command("printf '\\nCLIPPY_PLATFORM='; uname -sm")
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()?;
    ensure!(
        output.status.success(),
        "SSH platform probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let platform = text
        .lines()
        .find_map(|l| l.strip_prefix("CLIPPY_PLATFORM="))
        .context("missing platform probe")?;
    let arch = match platform.trim() {
        "Linux x86_64" => "x86_64",
        "Linux aarch64" | "Linux arm64" => "aarch64",
        _ => bail!("unsupported remote platform: {platform}"),
    };
    let bytes = if let Some(path) = agent_override {
        fs::read(path)?
    } else {
        bundled_agent(arch)?
    };
    ensure!(
        bytes.starts_with(b"\x7fELF"),
        "agent is not a Linux ELF executable"
    );
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(&bytes));
    let script = format!(
        "set -eu; umask 077; d=\"$HOME/.local/lib/clippy/{}\"; mkdir -p \"$d\"; t=$(mktemp \"$d/.install.XXXXXX\"); trap 'rm -f \"$t\"' EXIT HUP INT TERM; cat > \"$t\"; printf '%s  %s\\n' '{}' \"$t\" | sha256sum -c - >/dev/null; chmod 700 \"$t\"; mv -f \"$t\" \"$d/clippy-agent\"",
        env!("CARGO_PKG_VERSION"),
        hash
    );
    let mut child = connection.command(&script).stdin(Stdio::piped()).spawn()?;
    child.stdin.take().unwrap().write_all(&bytes)?;
    ensure!(child.wait()?.success(), "remote helper installation failed");
    println!(
        "Installed Clippy {} on {} ({arch}).",
        env!("CARGO_PKG_VERSION"),
        connection.host
    );
    Ok(())
}
fn bundled_agent(arch: &str) -> Result<Vec<u8>> {
    #[cfg(windows)]
    {
        let bytes = match arch {
            "x86_64" => include_bytes!(concat!(env!("OUT_DIR"), "/agent-x86_64")).as_slice(),
            _ => include_bytes!(concat!(env!("OUT_DIR"), "/agent-aarch64")).as_slice(),
        };
        if !bytes.is_empty() {
            return Ok(bytes.to_vec());
        }
    }
    let path = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("agents")
        .join(format!("clippy-agent-linux-{arch}"));
    fs::read(&path).with_context(|| {
        format!(
            "Linux helper not bundled; run scripts/package.ps1 or supply --agent PATH ({})",
            path.display()
        )
    })
}
pub fn cleanup(connection: &Connection, age: &str) -> Result<()> {
    let seconds = crate::transfer::duration_seconds(age)?;
    let script = format!(
        "exec \"$HOME/.local/lib/clippy/{}/clippy-agent\" clean {}",
        env!("CARGO_PKG_VERSION"),
        seconds
    );
    ensure!(
        connection.command(&script).status()?.success(),
        "remote cleanup failed"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn jump_and_config() {
        let c = Connection::parse(&a(&["-J", "bastion", "-i", "C:\\key file", "alias"])).unwrap();
        assert_eq!(c.host, "alias");
        assert_eq!(c.options.len(), 4);
    }
    #[test]
    fn refuses_transport_overrides() {
        for v in [
            vec!["-t", "host"],
            vec!["-oRemoteCommand=bad", "host"],
            vec!["host", "rm"],
            vec!["-N", "host"],
        ] {
            assert!(Connection::parse(&a(&v)).is_err());
        }
    }
}

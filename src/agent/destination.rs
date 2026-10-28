use anyhow::{Context, Result, ensure};
use std::fs;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Identity {
    pub pid: i32,
    pub start: String,
    pub uid: u32,
}

struct Process {
    identity: Identity,
    comm: String,
    parent: i32,
    group: i32,
    foreground: i32,
    tty: i64,
}

impl Process {
    fn read(pid: i32) -> Result<Self> {
        ensure!(pid > 0, "invalid foreground process");
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .context("invalid process metadata")?
            .1
            .split_whitespace()
            .collect();
        ensure!(fields.len() > 19, "incomplete process metadata");
        let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
        let uids: Vec<u32> = status
            .lines()
            .find_map(|s| s.strip_prefix("Uid:"))
            .context("missing process UID")?
            .split_whitespace()
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()?;
        ensure!(
            uids.len() == 4 && uids[1] == uids[3],
            "unsupported process credentials"
        );
        Ok(Self {
            identity: Identity {
                pid,
                start: fields[19].into(),
                uid: uids[1],
            },
            comm: fs::read_to_string(format!("/proc/{pid}/comm"))?
                .trim()
                .into(),
            parent: fields[1].parse()?,
            group: fields[2].parse()?,
            tty: fields[4].parse()?,
            foreground: fields[5].parse()?,
        })
    }
}

// Follow only the foreground terminal and explicit sudo/su monitor chains.
// Background jobs and other logins must never select the upload owner.
pub fn resolve(pid: i32) -> Result<Vec<Identity>> {
    let mut process = Process::read(pid)?;
    let mut chain = Vec::new();
    loop {
        ensure!(
            chain.len() < 32
                && !chain
                    .iter()
                    .any(|p: &Identity| p.pid == process.identity.pid),
            "ambiguous or excessively nested terminal session"
        );
        ensure!(
            process.tty != 0 && process.foreground > 0,
            "destination has no foreground terminal"
        );
        chain.push(process.identity.clone());
        if ["sudo", "su"].contains(&process.comm.as_str()) {
            let pid = process.identity.pid;
            let children = fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))?;
            let children: Vec<i32> = children
                .split_whitespace()
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()?;
            ensure!(
                children.len() == 1,
                "user switch is not ready or has an ambiguous destination"
            );
            let child = Process::read(children[0])?;
            ensure!(child.parent == pid, "user-switch process changed");
            process = child;
            continue;
        }
        if process.group != process.foreground {
            let next = Process::read(process.foreground)?;
            ensure!(next.tty == process.tty, "foreground terminal changed");
            let mut ancestor = next.parent;
            for _ in 0..32 {
                if ancestor == process.identity.pid || ancestor <= 1 {
                    break;
                }
                ancestor = Process::read(ancestor)?.parent;
            }
            ensure!(
                ancestor == process.identity.pid,
                "foreground process is outside the user-switch session"
            );
            process = next;
            continue;
        }
        ensure!(
            !["ssh", "mosh", "docker", "podman", "kubectl"].contains(&process.comm.as_str()),
            "pasting into nested SSH or containers is not supported yet"
        );
        ensure!(
            !process.comm.starts_with("tmux"),
            "tmux after a user switch is not supported yet"
        );
        return Ok(chain);
    }
}

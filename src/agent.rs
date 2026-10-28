use crate::{
    protocol::{self, Message},
    terminal::{PasteMode, paths_text},
    transfer,
};
use anyhow::{Context, Result, bail, ensure};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    thread,
};

type Output = Arc<Mutex<io::Stdout>>;
mod destination;
mod user_upload;
use user_upload::DestinationUpload;
fn send(out: &Output, msg: &Message) -> Result<()> {
    protocol::send(&mut *out.lock().unwrap(), msg)
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum Target {
    Direct {
        chain: Vec<destination::Identity>,
    },
    Tmux {
        pane: String,
        tty: String,
        chain: Vec<destination::Identity>,
    },
}
impl Target {
    fn uid(&self) -> u32 {
        let (Self::Direct { chain } | Self::Tmux { chain, .. }) = self;
        chain.last().unwrap().uid
    }
}
fn identity(fd: i32) -> Result<(i32, String, String)> {
    let pid = unsafe { libc::tcgetpgrp(fd) };
    ensure!(pid > 0, "cannot identify foreground process");
    process_identity(pid)
}
fn process_identity(pid: i32) -> Result<(i32, String, String)> {
    ensure!(pid > 0, "invalid foreground process group");
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let rest = stat.rsplit_once(')').context("invalid process metadata")?.1;
    let start = rest
        .split_whitespace()
        .nth(19)
        .context("missing process start time")?
        .to_string();
    let comm = fs::read_to_string(format!("/proc/{pid}/comm"))?
        .trim()
        .to_string();
    Ok((pid, start, comm))
}
fn target(fd: i32, tty: &str) -> Result<Target> {
    let (pid, _, comm) = identity(fd)?;
    if comm.starts_with("tmux") {
        let result=Command::new("tmux").args(["list-clients","-F","#{client_tty}|#{pane_id}|#{pane_tty}|#{client_prefix}|#{client_key_table}|#{pane_in_mode}|#{pane_pid}"]).output()?;
        ensure!(
            result.status.success(),
            "cannot inspect default tmux server"
        );
        let text = String::from_utf8(result.stdout)?;
        let fields = text
            .lines()
            .find(|l| l.split('|').next() == Some(tty))
            .context("tmux client not found; custom tmux sockets are not supported yet")?
            .split('|')
            .collect::<Vec<_>>();
        ensure!(
            fields.len() == 7 && fields[3] == "0" && fields[4] == "root" && fields[5] == "0",
            "leave tmux prefix/copy mode before pasting"
        );
        ensure!(
            fields[1].starts_with('%') && fields[1][1..].chars().all(|c| c.is_ascii_digit()),
            "invalid tmux pane"
        );
        // A tmux pane's slave PTY is not this helper's controlling terminal.
        // Read its foreground process group from the pane shell's /proc record.
        let shell_pid = fields[6].parse::<u32>().context("invalid tmux shell PID")?;
        let stat = fs::read_to_string(format!("/proc/{shell_pid}/stat"))?;
        let foreground = stat
            .rsplit_once(')')
            .context("invalid pane process metadata")?
            .1
            .split_whitespace()
            .nth(5)
            .context("missing pane foreground process group")?
            .parse::<i32>()?;
        Ok(Target::Tmux {
            pane: fields[1].into(),
            tty: fields[2].into(),
            chain: destination::resolve(foreground)?,
        })
    } else {
        Ok(Target::Direct {
            chain: destination::resolve(pid)?,
        })
    }
}
fn inject(
    target: &Target,
    text: &str,
    writer: &mut dyn Write,
    paste_mode: &Mutex<PasteMode>,
) -> Result<()> {
    match target {
        Target::Direct { .. } => {
            writer.write_all(&paste_mode.lock().unwrap().wrap(text))?;
            writer.flush()?;
        }
        Target::Tmux { pane, .. } => {
            let name = format!("clippy-{}", uuid::Uuid::new_v4());
            let mut child = Command::new("tmux")
                .args(["load-buffer", "-b", &name, "-"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()?;
            child.stdin.take().unwrap().write_all(text.as_bytes())?;
            ensure!(
                child.wait()?.success(),
                "could not create tmux paste buffer"
            );
            let result = Command::new("tmux")
                .args(["paste-buffer", "-d", "-p", "-b", &name, "-t", pane])
                .output();
            let _ = Command::new("tmux")
                .args(["delete-buffer", "-b", &name])
                .output();
            ensure!(result?.status.success(), "tmux paste failed");
        }
    }
    Ok(())
}
pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("receive-as") {
        ensure!(args.len() == 2, "receive-as requires a UID");
        return user_upload::receive_as(args[1].parse()?);
    }
    let home = std::env::var("HOME").context("HOME is required")?;
    match args.first().map(String::as_str) {
        Some("serve") => serve(&home),
        Some("clean") => {
            let seconds = args
                .get(1)
                .context("cleanup requires age in seconds")?
                .parse()?;
            let root = transfer::upload_root(Path::new(&home))?;
            println!(
                "Removed {} Clippy upload batches.",
                transfer::clean(&root, seconds)?
            );
            Ok(())
        }
        Some("--version") => {
            println!("clippy-agent {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => bail!("usage: clippy-agent serve | clean SECONDS | --version"),
    }
}
fn serve(home: &str) -> Result<()> {
    let output = Arc::new(Mutex::new(io::stdout()));
    {
        let mut out = output.lock().unwrap();
        out.write_all(protocol::MAGIC)?;
        out.flush()?;
    }
    send(
        &output,
        &Message::Hello {
            protocol: protocol::VERSION,
            version: env!("CARGO_PKG_VERSION").into(),
            home: home.into(),
        },
    )?;
    let mut input = io::stdin().lock();
    let Message::Start { term, cols, rows } = protocol::receive(&mut input)? else {
        bail!("expected Start");
    };
    ensure!(
        !term.is_empty() && term.len() < 100 && !term.chars().any(char::is_control),
        "invalid terminal name"
    );
    let root = transfer::upload_root(Path::new(home))?;
    let pair = native_pty_system().openpty(PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let fd = pair
        .master
        .as_raw_fd()
        .context("PTY descriptor unavailable")?;
    let mut name = [0 as libc::c_char; 256];
    ensure!(
        unsafe { libc::ptsname_r(fd, name.as_mut_ptr(), name.len()) } == 0,
        "cannot identify PTY"
    );
    let tty = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
        .to_str()?
        .to_string();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let mut command = CommandBuilder::new(shell);
    command.arg("-l");
    command.cwd(home);
    command.env("TERM", term);
    command.env("SSH_TTY", &tty);
    command.env("CLIPPY_SESSION", "1");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let mut killer = child.clone_killer();
    let output_exit = output.clone();
    thread::spawn(move || {
        if let Ok(status) = child.wait() {
            let _ = send(
                &output_exit,
                &Message::Exit {
                    code: status.exit_code(),
                },
            );
        }
    });
    let mode = Arc::new(Mutex::new(PasteMode::default()));
    let mode_reader = mode.clone();
    let mut reader = pair.master.try_clone_reader()?;
    let output_reader = output.clone();
    thread::spawn(move || {
        let mut buf = [0; 16384];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    mode_reader.lock().unwrap().observe(&buf[..n]);
                    if send(&output_reader, &Message::Output(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut writer = pair.master.take_writer()?;
    let mut upload: Option<(DestinationUpload, Target)> = None;
    send(&output, &Message::Started)?;
    let result = (|| -> Result<()> {
        loop {
            let message = protocol::receive(&mut input)?;
            if matches!(message, Message::Shutdown) {
                break;
            }
            let request_id = match &message {
                Message::Begin { id, .. } | Message::Commit { id } | Message::Cancel { id } => {
                    Some(id.clone())
                }
                _ => upload.as_ref().map(|u| u.0.id().to_owned()),
            };
            let action = (|| -> Result<()> {
                match message {
                    Message::Input(data) => {
                        ensure!(data.len() <= protocol::CHUNK, "oversized terminal input");
                        if upload.is_none() {
                            writer.write_all(&data)?;
                            writer.flush()?;
                        }
                    }
                    Message::Resize { cols, rows } => pair.master.resize(PtySize {
                        rows: rows.max(1),
                        cols: cols.max(1),
                        pixel_width: 0,
                        pixel_height: 0,
                    })?,
                    Message::Begin { id, files } => {
                        ensure!(upload.is_none(), "another upload is active");
                        let destination = target(fd, &tty)?;
                        upload = Some((
                            DestinationUpload::begin(&root, destination.uid(), &id, files)?,
                            destination,
                        ));
                        send(&output, &Message::Accepted { id })?;
                    }
                    Message::Chunk {
                        index,
                        offset,
                        data,
                    } => upload
                        .as_mut()
                        .context("no upload active")?
                        .0
                        .chunk(index, offset, &data)?,
                    Message::FinishFile { index, sha256 } => upload
                        .as_mut()
                        .context("no upload active")?
                        .0
                        .finish(index, sha256)?,
                    Message::Commit { id } => {
                        ensure!(
                            upload.as_ref().map(|u| u.0.id()) == Some(id.as_str()),
                            "upload ID mismatch"
                        );
                        let (batch, original) = upload.take().unwrap();
                        let paths = batch.commit()?;
                        let unchanged =
                            target(fd, &tty).map(|now| now == original).unwrap_or(false);
                        let (inserted, detail) = if unchanged {
                            match inject(&original, &paths_text(&paths), &mut *writer, &mode) {
                                Ok(()) => (true, String::new()),
                                Err(e) => {
                                    (false, format!("Saved files; path insertion failed: {e}"))
                                }
                            }
                        } else {
                            (false,"Saved files; destination changed during upload. Paths were not inserted.".into())
                        };
                        send(
                            &output,
                            &Message::Completed {
                                id,
                                paths,
                                inserted,
                                detail,
                            },
                        )?;
                    }
                    Message::Cancel { id } => {
                        if upload.as_ref().map(|u| u.0.id()) == Some(id.as_str()) {
                            upload.take();
                        }
                        send(
                            &output,
                            &Message::Completed {
                                id,
                                paths: vec![],
                                inserted: false,
                                detail: "Upload cancelled.".into(),
                            },
                        )?;
                    }
                    _ => bail!("unexpected client message"),
                }
                Ok(())
            })();
            if let Err(error) = action {
                upload.take();
                send(
                    &output,
                    &Message::Error {
                        id: request_id,
                        detail: format!("{error:#}"),
                    },
                )?;
            }
        }
        Ok(())
    })();
    drop(upload);
    drop(writer);
    let _ = killer.kill();
    // Only the login shell created by this helper is terminated; detached tmux servers are untouched.
    result
}

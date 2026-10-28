use super::{
    bridge::{Bridge, Event, State},
    clipboard::{Item, Source},
    console::{self, RawConsole},
};
use crate::{
    protocol::{self, Message},
    ssh::Connection,
};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    process::{Child, ChildStdin, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
type Sender = Arc<Mutex<ChildStdin>>;
fn send(writer: &Sender, msg: &Message) -> Result<()> {
    protocol::send(&mut *writer.lock().unwrap(), msg)
}
enum LocalEvent {
    Remote(Message),
    Disconnected(String),
    UploadFailed { id: String, detail: String },
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn connect(connection: Connection) -> Result<()> {
    ensure!(
        std::env::var_os("WT_SESSION").is_some(),
        "run clippy ssh from Windows Terminal (PowerShell), not a redirected or legacy console"
    );
    let mut process = ChildGuard(
        connection
            .serve()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start Windows OpenSSH")?,
    );
    let writer = Arc::new(Mutex::new(process.0.stdin.take().unwrap()));
    let mut reader = process.0.stdout.take().unwrap();
    protocol::handshake(&mut reader)?;
    let Message::Hello {
        protocol: version,
        version: agent_version,
        ..
    } = protocol::receive(&mut reader)?
    else {
        bail!("invalid helper greeting");
    };
    ensure!(
        version == protocol::VERSION && agent_version == env!("CARGO_PKG_VERSION"),
        "helper version mismatch; run clippy setup with this host's connection options"
    );
    let bridge = Bridge::start()?;
    let _raw = RawConsole::enter()?;
    let (cols, rows) = console::size();
    send(
        &writer,
        &Message::Start {
            term: "xterm-256color".into(),
            cols,
            rows,
        },
    )?;
    let (tx, rx) = mpsc::channel();
    let remote_tx = tx.clone();
    thread::spawn(move || {
        loop {
            match protocol::receive(&mut reader) {
                Ok(Message::Output(data)) => {
                    let mut out = std::io::stdout().lock();
                    if out.write_all(&data).and_then(|_| out.flush()).is_err() {
                        break;
                    }
                }
                Ok(message) => {
                    if remote_tx.send(LocalEvent::Remote(message)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = remote_tx.send(LocalEvent::Disconnected(format!("{e:#}")));
                    break;
                }
            }
        }
    });
    let stop = Arc::new(AtomicBool::new(false));
    let input_stop = stop.clone();
    let input_writer = writer.clone();
    let state = bridge.state.clone();
    thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        let mut buffer = [0; 4096];
        while !input_stop.load(Ordering::Relaxed) {
            match input.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if state.busy.load(Ordering::SeqCst) {
                        if buffer[..n].contains(&3) {
                            state.cancel.store(true, Ordering::SeqCst);
                        }
                        continue;
                    }
                    if send(&input_writer, &Message::Input(buffer[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut active: Option<String> = None;
    let mut pending: Option<Vec<Item>> = None;
    let mut upload_thread: Option<thread::JoinHandle<()>> = None;
    let mut cancellation_sent = false;
    let mut last_size = (cols, rows);
    let mut resized = Instant::now();
    let result = (|| -> Result<()> {
        loop {
            while let Ok(event) = bridge.events.try_recv() {
                match event {
                    Event::Paste { items } => {
                        if active.is_some() {
                            continue;
                        }
                        if bridge.state.cancel.load(Ordering::SeqCst) {
                            bridge.state.show("Upload cancelled.", 2);
                            bridge.state.finish();
                            continue;
                        }
                        let id = uuid::Uuid::new_v4().to_string();
                        bridge
                            .state
                            .show("Uploading — terminal input paused. Ctrl+C cancels.", 3600);
                        let specs = items.iter().map(|i| i.spec.clone()).collect();
                        pending = Some(items);
                        active = Some(id.clone());
                        cancellation_sent = false;
                        send(&writer, &Message::Begin { id, files: specs })?;
                    }
                    Event::Cancel => bridge.state.cancel.store(true, Ordering::SeqCst),
                    Event::Error(detail) => bridge.state.show(detail, 8),
                }
            }
            if bridge.state.cancel.load(Ordering::SeqCst) && !cancellation_sent {
                if let Some(id) = &active {
                    // Finish the in-flight frame before cancellation; no worker may send after Cancel.
                    if let Some(worker) = upload_thread.take() {
                        let _ = worker.join();
                    }
                    pending.take();
                    send(&writer, &Message::Cancel { id: id.clone() })?;
                    cancellation_sent = true;
                } else if !bridge.state.busy.load(Ordering::SeqCst) {
                    bridge.state.finish();
                }
            }
            if resized.elapsed() > Duration::from_millis(200) {
                let size = console::size();
                if size != last_size {
                    send(
                        &writer,
                        &Message::Resize {
                            cols: size.0,
                            rows: size.1,
                        },
                    )?;
                    last_size = size;
                }
                resized = Instant::now();
            }
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(LocalEvent::Remote(Message::Started)) => {}
                Ok(LocalEvent::Remote(Message::Accepted { id })) => {
                    if active.as_ref() != Some(&id) || cancellation_sent {
                        continue;
                    }
                    let Some(items) = pending.take() else {
                        continue;
                    };
                    let worker_writer = writer.clone();
                    let worker_state = bridge.state.clone();
                    let worker_tx = tx.clone();
                    upload_thread = Some(thread::spawn(move || {
                        if let Err(e) = upload(&worker_writer, &worker_state, &id, items) {
                            let _ = worker_tx.send(LocalEvent::UploadFailed {
                                id,
                                detail: format!("{e:#}"),
                            });
                        }
                    }));
                }
                Ok(LocalEvent::Remote(Message::Completed {
                    id,
                    paths,
                    inserted,
                    detail,
                })) => {
                    if active.as_ref() != Some(&id) {
                        continue;
                    }
                    if let Some(worker) = upload_thread.take() {
                        let _ = worker.join();
                    }
                    if !paths.is_empty() {
                        save_receipt(&connection.host, &id, &paths, inserted, &detail);
                    }
                    if inserted && detail.is_empty() {
                        // The inserted path is the success feedback; dismiss upload progress.
                        bridge.state.show("", 0);
                    } else if paths.is_empty() {
                        bridge.state.show(detail, 10);
                    } else {
                        bridge.state.show(
                            format!("{detail}\r\nRun clippy last to see the saved paths."),
                            10,
                        );
                    }
                    active = None;
                    pending = None;
                    bridge.state.finish();
                }
                Ok(LocalEvent::Remote(Message::Error { id, detail })) => {
                    if id.as_ref() == active.as_ref() || id.is_none() {
                        bridge.state.cancel.store(true, Ordering::SeqCst);
                        if let Some(worker) = upload_thread.take() {
                            let _ = worker.join();
                        }
                        bridge.state.show(detail, 10);
                        active = None;
                        pending = None;
                        bridge.state.finish();
                    }
                }
                Ok(LocalEvent::UploadFailed { id, detail }) => {
                    if active.as_ref() == Some(&id) && !cancellation_sent {
                        bridge.state.show(detail, 10);
                        bridge.state.cancel.store(true, Ordering::SeqCst);
                    }
                }
                Ok(LocalEvent::Remote(Message::Exit { code })) => {
                    if code != 0 {
                        bail!("remote shell exited with status {code}");
                    }
                    break;
                }
                Ok(LocalEvent::Disconnected(detail)) => bail!("SSH connection ended: {detail}"),
                Ok(LocalEvent::Remote(_)) => bail!("unexpected server message"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    })();
    stop.store(true, Ordering::SeqCst);
    bridge.state.cancel.store(true, Ordering::SeqCst);
    // Killing only this owned SSH process unblocks a worker if the network died mid-write.
    let _ = process.0.kill();
    if let Some(worker) = upload_thread.take() {
        let _ = worker.join();
    }
    result
}

fn upload(writer: &Sender, state: &State, id: &str, items: Vec<Item>) -> Result<()> {
    let total = items
        .iter()
        .try_fold(0u64, |sum, i| sum.checked_add(i.spec.size))
        .context("batch size overflow")?;
    let mut sent = 0u64;
    for (index, mut item) in items.into_iter().enumerate() {
        let mut offset = 0u64;
        let mut hash = Sha256::new();
        let mut buf = vec![0; protocol::CHUNK];
        loop {
            ensure!(!state.cancel.load(Ordering::SeqCst), "Upload cancelled.");
            let n = match &mut item.source {
                Source::File { file, .. } => file.read(&mut buf)?,
                Source::Bytes(cursor) => cursor.read(&mut buf)?,
            };
            if n == 0 {
                break;
            }
            ensure!(
                offset + n as u64 <= item.spec.size,
                "source file changed size during upload"
            );
            send(
                writer,
                &Message::Chunk {
                    index,
                    offset,
                    data: buf[..n].to_vec(),
                },
            )?;
            hash.update(&buf[..n]);
            offset += n as u64;
            sent += n as u64;
            state.show(
                format!(
                    "Uploading {}% — input paused. Ctrl+C cancels.",
                    if total == 0 {
                        100
                    } else {
                        (sent as f64 / total as f64 * 100.0) as u32
                    }
                ),
                3600,
            );
        }
        ensure!(
            offset == item.spec.size,
            "source file changed size during upload"
        );
        if let Source::File {
            file,
            path,
            modified,
        } = &item.source
        {
            let metadata = file.metadata()?;
            ensure!(
                metadata.len() == item.spec.size && metadata.modified().ok() == *modified,
                "source changed during upload: {}",
                path.display()
            );
        }
        ensure!(!state.cancel.load(Ordering::SeqCst), "Upload cancelled.");
        send(
            writer,
            &Message::FinishFile {
                index,
                sha256: hash.finalize().into(),
            },
        )?;
    }
    ensure!(!state.cancel.load(Ordering::SeqCst), "Upload cancelled.");
    send(writer, &Message::Commit { id: id.into() })
}
fn receipt_path() -> Result<std::path::PathBuf> {
    Ok(
        std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA missing")?)
            .join("Clippy")
            .join("last-upload.json"),
    )
}
fn save_receipt(host: &str, id: &str, paths: &[String], inserted: bool, detail: &str) {
    if let Ok(path) = receipt_path() {
        if std::fs::create_dir_all(path.parent().unwrap()).is_ok() {
            let value = serde_json::json!({"host":host,"id":id,"paths":paths,"inserted":inserted,"detail":detail});
            let _ = std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap());
        }
    }
}
pub fn last() -> Result<()> {
    println!(
        "{}",
        std::fs::read_to_string(receipt_path()?).context("no completed upload receipt yet")?
    );
    Ok(())
}

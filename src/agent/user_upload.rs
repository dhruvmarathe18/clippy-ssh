use crate::{
    protocol::{self, FileSpec, Message},
    transfer::{self, Upload},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::{CStr, CString},
    io,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

pub enum DestinationUpload {
    Local(Upload),
    User(UserUpload),
}

impl DestinationUpload {
    pub fn begin(root: &Path, uid: u32, id: &str, files: Vec<FileSpec>) -> Result<Self> {
        if uid == unsafe { libc::geteuid() } {
            Ok(Self::Local(Upload::begin(root, id, files)?))
        } else {
            Ok(Self::User(UserUpload::begin(uid, id, files)?))
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Local(u) => &u.id,
            Self::User(u) => &u.id,
        }
    }
    pub fn chunk(&mut self, index: usize, offset: u64, data: &[u8]) -> Result<()> {
        match self {
            Self::Local(u) => u.chunk(index, offset, data),
            Self::User(u) => u.send(&Message::Chunk {
                index,
                offset,
                data: data.into(),
            }),
        }
    }
    pub fn finish(&mut self, index: usize, sha256: [u8; 32]) -> Result<()> {
        match self {
            Self::Local(u) => u.finish(index, sha256),
            Self::User(u) => u.send(&Message::FinishFile { index, sha256 }),
        }
    }
    pub fn commit(self) -> Result<Vec<String>> {
        match self {
            Self::Local(u) => u.commit(),
            Self::User(mut u) => {
                u.send(&Message::Commit { id: u.id.clone() })?;
                match u.receive()? {
                    Message::Completed { id, paths, .. } if id == u.id => Ok(paths),
                    _ => bail!("invalid user-upload completion"),
                }
            }
        }
    }
}

pub struct UserUpload {
    id: String,
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
}

impl UserUpload {
    fn begin(uid: u32, id: &str, files: Vec<FileSpec>) -> Result<Self> {
        let executable = std::env::current_exe()?;
        let mut command = if unsafe { libc::geteuid() } == 0 {
            Command::new(&executable)
        } else {
            // No password prompt or sudoers changes. The helper immediately
            // drops to the foreground UID before reading any upload frames.
            let mut c = Command::new("sudo");
            c.args(["-n", "--"]).arg(&executable);
            c
        };
        let mut child = command
            .arg("receive-as")
            .arg(uid.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let mut upload = Self {
            id: id.into(),
            child,
            input,
            output,
        };
        let hello = upload.receive().context(
            "paste after su/sudo requires noninteractive sudo permission for the helper",
        )?;
        ensure!(
            matches!(
                hello,
                Message::Hello {
                    protocol: protocol::VERSION,
                    ..
                }
            ),
            "invalid user-upload greeting"
        );
        upload.send(&Message::Begin {
            id: id.into(),
            files,
        })?;
        ensure!(
            matches!(upload.receive()?, Message::Accepted { id: accepted } if accepted == id),
            "user-upload not accepted"
        );
        Ok(upload)
    }
    fn send(&mut self, message: &Message) -> Result<()> {
        protocol::send(
            self.input.as_mut().context("user-upload input closed")?,
            message,
        )
    }
    fn receive(&mut self) -> Result<Message> {
        match protocol::receive(&mut self.output)? {
            Message::Error { detail, .. } => bail!("{detail}"),
            message => Ok(message),
        }
    }
}
impl Drop for UserUpload {
    fn drop(&mut self) {
        // EOF lets the receiver remove an unfinished staging batch as its owner.
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn become_user(uid: u32) -> Result<String> {
    // Called before this process creates threads. Copy passwd fields before
    // calling initgroups, which may perform further NSS lookups.
    let (name, home, gid) = unsafe {
        let entry = libc::getpwuid(uid);
        ensure!(!entry.is_null(), "foreground user has no passwd entry");
        (
            CString::new(CStr::from_ptr((*entry).pw_name).to_bytes())?,
            CStr::from_ptr((*entry).pw_dir).to_str()?.to_owned(),
            (*entry).pw_gid,
        )
    };
    unsafe {
        ensure!(
            libc::geteuid() == 0 || libc::geteuid() == uid,
            "cannot switch upload owner"
        );
        if libc::geteuid() == 0 {
            ensure!(
                libc::initgroups(name.as_ptr(), gid) == 0,
                "cannot set upload groups: {}",
                io::Error::last_os_error()
            );
            ensure!(
                libc::setresgid(gid, gid, gid) == 0,
                "cannot set upload GID: {}",
                io::Error::last_os_error()
            );
            ensure!(
                libc::setresuid(uid, uid, uid) == 0,
                "cannot set upload UID: {}",
                io::Error::last_os_error()
            );
        }
        ensure!(
            libc::getuid() == uid && libc::geteuid() == uid,
            "upload UID mismatch"
        );
        libc::umask(0o077);
    }
    ensure!(Path::new(&home).is_absolute(), "user home is not absolute");
    Ok(home)
}

pub fn receive_as(uid: u32) -> Result<()> {
    let home = become_user(uid)?;
    let mut output = io::stdout().lock();
    protocol::send(
        &mut output,
        &Message::Hello {
            protocol: protocol::VERSION,
            version: env!("CARGO_PKG_VERSION").into(),
            home: home.clone(),
        },
    )?;
    let result = (|| -> Result<()> {
        let mut input = io::stdin().lock();
        let Message::Begin { id, files } = protocol::receive(&mut input)? else {
            bail!("expected user-upload Begin");
        };
        let root = transfer::upload_root(Path::new(&home))?;
        let mut upload = Upload::begin(&root, &id, files)?;
        protocol::send(&mut output, &Message::Accepted { id: id.clone() })?;
        loop {
            match protocol::receive(&mut input)? {
                Message::Chunk {
                    index,
                    offset,
                    data,
                } => upload.chunk(index, offset, &data)?,
                Message::FinishFile { index, sha256 } => upload.finish(index, sha256)?,
                Message::Commit { id: commit } => {
                    ensure!(commit == id, "user-upload ID mismatch");
                    let paths = upload.commit()?;
                    protocol::send(
                        &mut output,
                        &Message::Completed {
                            id,
                            paths,
                            inserted: false,
                            detail: String::new(),
                        },
                    )?;
                    return Ok(());
                }
                _ => bail!("unexpected user-upload message"),
            }
        }
    })();
    if let Err(error) = result {
        let _ = protocol::send(
            &mut output,
            &Message::Error {
                id: None,
                detail: format!("{error:#}"),
            },
        );
    }
    Ok(())
}

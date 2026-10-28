use anyhow::{Context, Result, bail};
use bincode::Options;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const VERSION: u16 = 3;
pub const CHUNK: usize = 64 * 1024;
pub const MAX_FRAME: usize = 1024 * 1024;
pub const MAX_FILES: usize = 1024;
pub const MAGIC: &[u8] = b"CLIPPY-SSH\0\x01";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSpec {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Message {
    Hello {
        protocol: u16,
        version: String,
        home: String,
    },
    Start {
        term: String,
        cols: u16,
        rows: u16,
    },
    Started,
    Input(Vec<u8>),
    Output(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    Begin {
        id: String,
        files: Vec<FileSpec>,
    },
    Accepted {
        id: String,
    },
    Chunk {
        index: usize,
        offset: u64,
        data: Vec<u8>,
    },
    FinishFile {
        index: usize,
        sha256: [u8; 32],
    },
    Commit {
        id: String,
    },
    Cancel {
        id: String,
    },
    Completed {
        id: String,
        paths: Vec<String>,
        inserted: bool,
        detail: String,
    },
    Error {
        id: Option<String>,
        detail: String,
    },
    Exit {
        code: u32,
    },
    Shutdown,
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_FRAME as u64)
        .reject_trailing_bytes()
}
pub fn send(mut writer: impl Write, message: &Message) -> Result<()> {
    let bytes = codec().serialize(message)?;
    if bytes.len() > MAX_FRAME {
        bail!("protocol frame exceeds limit");
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
pub fn receive(mut reader: impl Read) -> Result<Message> {
    let mut header = [0; 4];
    reader
        .read_exact(&mut header)
        .context("SSH stream closed")?;
    let size = u32::from_be_bytes(header) as usize;
    if size == 0 || size > MAX_FRAME {
        bail!("invalid protocol frame length: {size}");
    }
    let mut data = vec![0; size];
    reader
        .read_exact(&mut data)
        .context("truncated protocol frame")?;
    Ok(codec().deserialize(&data)?)
}
pub fn handshake(mut reader: impl Read) -> Result<()> {
    let mut matched = 0;
    for _ in 0..65536 {
        let mut b = [0];
        reader
            .read_exact(&mut b)
            .context("helper did not start; run clippy setup HOST")?;
        if b[0] == MAGIC[matched] {
            matched += 1;
        } else {
            matched = usize::from(b[0] == MAGIC[0]);
        }
        if matched == MAGIC.len() {
            return Ok(());
        }
    }
    bail!("helper handshake missing or too much startup output")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_and_fragmentation() {
        let mut buf = vec![];
        send(&mut buf, &Message::Input(vec![0, 255, 10])).unwrap();
        assert!(matches!(receive(&buf[..]).unwrap(),Message::Input(v) if v==[0,255,10]));
        assert!(receive(&buf[..buf.len() - 1]).is_err());
    }
    #[test]
    fn rejects_large_frames_before_allocating() {
        assert!(receive(&[255u8; 4][..]).is_err());
    }
    #[test]
    fn rejects_trailing_data() {
        let mut b = vec![];
        send(&mut b, &Message::Shutdown).unwrap();
        b.push(0);
        let n = (b.len() - 4) as u32;
        b[..4].copy_from_slice(&n.to_be_bytes());
        assert!(receive(&b[..]).is_err());
    }
    #[test]
    fn accepts_banner() {
        let b = [b"Welcome\r\n".as_slice(), MAGIC].concat();
        handshake(&b[..]).unwrap();
    }
}

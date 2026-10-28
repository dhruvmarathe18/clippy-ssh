use anyhow::{Context, Result, ensure};
use bincode::Options;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        Storage::FileSystem::*,
        System::{Pipes::*, Threading::*},
    },
    core::{PCWSTR, PWSTR},
};

const LIMIT: u64 = 16 * 1024;
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Action {
    Status,
    Pause,
    Resume,
    Stop,
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    Register {
        busy: bool,
    },
    Poll {
        active: bool,
        window: isize,
        finished: bool,
    },
    Unregister,
    Control(Action),
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Hello {
        version: String,
    },
    Registered,
    Paste {
        sequence: u32,
        window: isize,
    },
    Cancel,
    Idle,
    Status {
        pid: u32,
        paused: bool,
        sessions: usize,
    },
}
pub fn send<T: Serialize>(file: &mut File, message: &T) -> Result<()> {
    let bytes = bincode::DefaultOptions::new()
        .with_limit(LIMIT)
        .serialize(message)?;
    ensure!(bytes.len() <= LIMIT as usize, "oversized local message");
    file.write_all(&(bytes.len() as u32).to_le_bytes())?;
    file.write_all(&bytes)?;
    file.flush()?;
    Ok(())
}
pub fn receive<T: DeserializeOwned>(file: &mut File) -> Result<T> {
    let mut header = [0; 4];
    file.read_exact(&mut header)?;
    let len = u32::from_le_bytes(header) as usize;
    ensure!(
        len > 0 && len <= LIMIT as usize,
        "invalid local frame length"
    );
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes)?;
    Ok(bincode::DefaultOptions::new()
        .with_limit(LIMIT)
        .reject_trailing_bytes()
        .deserialize(&bytes)?)
}
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
pub struct OwnedHandle(pub HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn logon_sid() -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let token = OwnedHandle(token);
        let mut size = 0;
        let _ = GetTokenInformation(token.0, TokenLogonSid, None, 0, &mut size);
        ensure!(size > 0, "cannot identify Windows logon");
        let mut storage = vec![0u64; (size as usize).div_ceil(8)];
        GetTokenInformation(
            token.0,
            TokenLogonSid,
            Some(storage.as_mut_ptr().cast()),
            size,
            &mut size,
        )?;
        let groups = &*(storage.as_ptr() as *const TOKEN_GROUPS);
        ensure!(groups.GroupCount == 1, "missing logon SID");
        let mut value = PWSTR::null();
        ConvertSidToStringSidW(groups.Groups[0].Sid, &mut value)?;
        let result = value.to_string();
        LocalFree(Some(HLOCAL(value.0.cast())));
        Ok(result?)
    }
}
pub fn sibling(name: &str) -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .context("executable folder missing")?
        .join(name))
}
pub fn endpoint() -> Result<String> {
    endpoint_for(&std::env::current_exe()?)
}
pub fn endpoint_for(executable: &Path) -> Result<String> {
    // Different installations/development builds never steal each other's connections.
    let directory = executable
        .parent()
        .unwrap()
        .canonicalize()?
        .to_string_lossy()
        .to_lowercase();
    let hash = hex::encode(Sha256::digest(directory.as_bytes()));
    Ok(format!(
        r"\\.\pipe\Clippy-{}-{}-{}",
        logon_sid()?,
        env!("CARGO_PKG_VERSION"),
        &hash[..16]
    ))
}
pub fn create_listener(first: bool) -> Result<File> {
    let name = wide(&endpoint()?);
    let descriptor_text = wide(&format!("D:P(A;;GA;;;{})", logon_sid()?));
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(descriptor_text.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let mut flags = PIPE_ACCESS_DUPLEX;
        if first {
            flags |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let handle = CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            flags,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            64,
            32768,
            32768,
            1000,
            Some(&attributes),
        );
        let error = windows::core::Error::from_win32();
        LocalFree(Some(HLOCAL(descriptor.0)));
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "cannot create background endpoint: {error}"
        );
        Ok(File::from_raw_handle(handle.0))
    }
}
pub fn accept(file: &File) -> Result<()> {
    unsafe {
        if let Err(error) = ConnectNamedPipe(HANDLE(file.as_raw_handle()), None) {
            if error.code() != windows::core::HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) {
                return Err(error.into());
            }
        }
    }
    Ok(())
}
fn process_path(pid: u32) -> Result<PathBuf> {
    unsafe {
        let process = OwnedHandle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?);
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )?;
        Ok(PathBuf::from(String::from_utf16(&buf[..len as usize])?))
    }
}
fn same_path(actual: &Path, expected: &Path) -> Result<bool> {
    Ok(actual
        .canonicalize()?
        .to_string_lossy()
        .eq_ignore_ascii_case(&expected.canonicalize()?.to_string_lossy()))
}
pub fn validate_client(file: &File) -> Result<u32> {
    let mut pid = 0;
    unsafe {
        GetNamedPipeClientProcessId(HANDLE(file.as_raw_handle()), &mut pid)?;
    }
    ensure!(
        same_path(&process_path(pid)?, &sibling("clippy.exe")?)?,
        "unrecognized background client"
    );
    Ok(pid)
}
pub fn connect() -> Result<File> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0)
        .open(endpoint()?)
        .context("background app is not running")?;
    let mut pid = 0;
    unsafe {
        GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut pid)?;
    }
    ensure!(
        same_path(&process_path(pid)?, &sibling("clippy-background.exe")?)?,
        "unrecognized background server"
    );
    ensure!(
        matches!(receive::<Response>(&mut file)?, Response::Hello {version} if version == env!("CARGO_PKG_VERSION")),
        "background version mismatch"
    );
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};
    #[test]
    fn local_protocol_bounds_and_round_trip() {
        let mut file = tempfile::tempfile().unwrap();
        send(
            &mut file,
            &Request::Poll {
                active: true,
                window: 123,
                finished: false,
            },
        )
        .unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        assert!(matches!(
            receive::<Request>(&mut file).unwrap(),
            Request::Poll {
                active: true,
                window: 123,
                finished: false
            }
        ));
        file.set_len(0).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(&((LIMIT + 1) as u32).to_le_bytes()).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        assert!(receive::<Request>(&mut file).is_err());
    }
}

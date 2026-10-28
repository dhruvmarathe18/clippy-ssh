use super::{ipc, policy};
use crate::integration::profile_block;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};
use windows::{
    Win32::{
        Foundation::ERROR_FILE_NOT_FOUND,
        System::{Com::CoTaskMemFree, Registry::*, Threading::CREATE_NO_WINDOW},
        UI::Shell::*,
    },
    core::{PCWSTR, w},
};

const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("ClippySSH");
#[derive(Serialize, Deserialize)]
struct Manifest {
    client: PathBuf,
    background: PathBuf,
    script: PathBuf,
    profiles: Vec<PathBuf>,
    startup: String,
    #[serde(default)]
    policy_changes: Vec<policy::Change>,
}

fn root() -> Result<PathBuf> {
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA missing")?);
    let root = base.join("Clippy");
    fs::create_dir_all(&root)?;
    ensure!(
        root.canonicalize()?.starts_with(base.canonicalize()?),
        "installation directory leaves LOCALAPPDATA"
    );
    Ok(root)
}
fn documents() -> Result<PathBuf> {
    unsafe {
        let value = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None)?;
        let text = value.to_string();
        CoTaskMemFree(Some(value.0.cast()));
        Ok(PathBuf::from(text?))
    }
}
fn startup_value() -> Result<Option<String>> {
    unsafe {
        let mut size = 0;
        let result = RegGetValueW(
            HKEY_CURRENT_USER,
            RUN,
            VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        );
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        result.ok()?;
        let mut data = vec![0u16; (size as usize).div_ceil(2)];
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN,
            VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()?;
        let end = data.iter().position(|v| *v == 0).unwrap_or(data.len());
        Ok(Some(String::from_utf16(&data[..end])?))
    }
}
fn set_startup(value: Option<&str>) -> Result<()> {
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let result = if let Some(value) = value {
            let data: Vec<u8> = value
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect();
            RegSetValueExW(key, VALUE, None, REG_SZ, Some(&data))
        } else {
            RegDeleteValueW(key, VALUE)
        };
        let _ = RegCloseKey(key);
        if value.is_none() && result == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        result.ok()?;
    }
    Ok(())
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("file has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".clippy-{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, bytes)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
}
fn decode(bytes: &[u8]) -> Result<(String, Encoding)> {
    if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
        ensure!(bytes.len() % 2 == 0, "invalid UTF-16 profile");
        let units = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>();
        return Ok((String::from_utf16(&units)?, Encoding::Utf16Le));
    }
    if let Some(bytes) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return Ok((String::from_utf8(bytes.into())?, Encoding::Utf8Bom));
    }
    Ok((
        String::from_utf8(bytes.into())
            .context("profile must be UTF-8 or UTF-16LE; it was not changed")?,
        if bytes.is_empty() {
            Encoding::Utf8Bom
        } else {
            Encoding::Utf8
        },
    ))
}
fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [vec![0xef, 0xbb, 0xbf], text.as_bytes().to_vec()].concat(),
        Encoding::Utf16Le => [
            vec![0xff, 0xfe],
            text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ]
        .concat(),
    }
}
fn edit_profile(path: &Path, script: Option<&Path>) -> Result<(Vec<u8>, Vec<u8>)> {
    let old = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => return Err(error.into()),
    };
    let (text, encoding) = decode(&old)?;
    let script_text = script.map(|p| p.to_string_lossy().into_owned());
    let changed = profile_block(&text, script_text.as_deref())?;
    let new = encode(&changed, encoding);
    Ok((old, new))
}
fn powershell_script(client: &Path) -> String {
    let client = client.to_string_lossy().replace('\'', "''");
    format!(
        r#"# Clippy SSH. Generated by clippy install; ordinary ssh.exe remains available.
if (!(Test-Path Alias:\ssh) -and !(Test-Path Function:\ssh)) {{
    function global:ssh {{
        if (Test-Path -LiteralPath '{client}') {{ & '{client}' ssh-auto @args }}
        else {{ & ssh.exe @args }}
        $global:LASTEXITCODE = $LASTEXITCODE
    }}
}}
if (!(Test-Path Alias:\clippy) -and !(Test-Path Function:\clippy)) {{
    function global:clippy {{
        & '{client}' @args
        $global:LASTEXITCODE = $LASTEXITCODE
    }}
}}
"#
    )
}
pub fn preflight(enable_profiles: bool) -> Result<()> {
    let ssh = Command::new("ssh.exe").arg("-V").creation_flags(CREATE_NO_WINDOW.0)
        .output().context("Windows OpenSSH Client is missing. Enable OpenSSH Client in Windows Settings > Optional features, then run Setup again.")?;
    ensure!(
        ssh.status.success(),
        "Windows OpenSSH Client could not start"
    );
    policy::plan(enable_profiles)?;
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA missing")?)
        .join("Clippy");
    let script = base.join("shell/ssh.ps1");
    for folder in ["WindowsPowerShell", "PowerShell"] {
        edit_profile(
            &documents()?.join(folder).join("profile.ps1"),
            Some(&script),
        )?;
    }
    if let Some(startup) = startup_value()? {
        let previous: Manifest =
            serde_json::from_slice(&fs::read(base.join("installation.json")).context(
                "ClippySSH startup entry exists without an installation record; it was not changed",
            )?)?;
        ensure!(
            startup == previous.startup,
            "ClippySSH startup entry belongs to another installation; it was not changed"
        );
    }
    Ok(())
}

struct FileBackup {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}
impl FileBackup {
    fn capture(path: &Path) -> Result<Self> {
        let bytes = match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path: path.to_owned(),
            bytes,
        })
    }
    fn restore(&self) -> Result<()> {
        if let Some(bytes) = &self.bytes {
            atomic_write(&self.path, bytes)?;
        } else if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

pub fn install(enable_profiles: bool) -> Result<()> {
    preflight(enable_profiles)?;
    let planned_policy = policy::plan(enable_profiles)?;
    let root = root()?;
    let source_client = std::env::current_exe()?;
    let source_background = ipc::sibling("clippy-background.exe")?;
    let client_bytes = fs::read(source_client)?;
    let background_bytes =
        fs::read(source_background).context("clippy-background.exe must be beside clippy.exe")?;
    let mut hash = Sha256::new();
    hash.update(&client_bytes);
    hash.update(&background_bytes);
    let digest = hex::encode(hash.finalize());
    let version =
        root.join("versions")
            .join(format!("{}-{}", env!("CARGO_PKG_VERSION"), &digest[..12]));
    fs::create_dir_all(&version)?;
    ensure!(
        version.canonicalize()?.starts_with(root.canonicalize()?),
        "version directory leaves Clippy installation"
    );
    let client = version.join("clippy.exe");
    let background = version.join("clippy-background.exe");
    for (path, bytes) in [(&client, &client_bytes), (&background, &background_bytes)] {
        if path.exists() {
            ensure!(
                fs::read(path)? == *bytes,
                "installed binary differs from its content-addressed version"
            );
        } else {
            atomic_write(path, bytes)?;
        }
    }
    let manifest_path = root.join("installation.json");
    let previous = fs::read(&manifest_path)
        .ok()
        .and_then(|v| serde_json::from_slice::<Manifest>(&v).ok());
    let startup = format!("\"{}\"", background.display());
    let old_startup = startup_value()?;
    ensure!(
        old_startup.is_none()
            || old_startup.as_deref() == Some(&startup)
            || previous.as_ref().map(|p| p.startup.as_str()) == old_startup.as_deref(),
        "ClippySSH startup entry belongs to another installation; it was not changed"
    );
    let script = root.join("shell").join("ssh.ps1");
    let documents = documents()?;
    let profiles = vec![
        documents.join("WindowsPowerShell").join("profile.ps1"),
        documents.join("PowerShell").join("profile.ps1"),
    ];
    let edits = profiles
        .iter()
        .map(|p| edit_profile(p, Some(&script)))
        .collect::<Result<Vec<_>>>()?;
    let mut backups = profiles
        .iter()
        .map(|p| FileBackup::capture(p))
        .collect::<Result<Vec<_>>>()?;
    backups.push(FileBackup::capture(&script)?);
    backups.push(FileBackup::capture(&manifest_path)?);
    let mut manifest = Manifest {
        client: client.clone(),
        background,
        script,
        profiles: profiles.clone(),
        startup: startup.clone(),
        policy_changes: previous.map(|p| p.policy_changes).unwrap_or_default(),
    };
    let mut applied_policy = Vec::new();
    let installed = (|| -> Result<()> {
        policy::apply(&planned_policy, &mut applied_policy)?;
        manifest
            .policy_changes
            .extend(applied_policy.iter().cloned());
        atomic_write(
            &manifest.script,
            &encode(&powershell_script(&client), Encoding::Utf8Bom),
        )?;
        for (path, (old, new)) in profiles.iter().zip(&edits) {
            if old == new {
                continue;
            }
            if !old.is_empty() {
                atomic_write(
                    &path.with_extension(format!("clippy-backup-{}", uuid::Uuid::new_v4())),
                    old,
                )?;
            }
            atomic_write(path, new)?;
        }
        set_startup(Some(&startup))?;
        atomic_write(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
        ensure!(
            Command::new(&client)
                .args(["background", "start"])
                .creation_flags(CREATE_NO_WINDOW.0)
                .status()?
                .success(),
            "installed background app could not start"
        );
        Ok(())
    })();
    if let Err(error) = installed {
        let mut rollback_errors = Vec::new();
        for backup in backups.iter().rev() {
            if let Err(e) = backup.restore() {
                rollback_errors.push(e.to_string());
            }
        }
        if let Err(e) = set_startup(old_startup.as_deref()) {
            rollback_errors.push(e.to_string());
        }
        if let Err(e) = policy::restore(&applied_policy) {
            rollback_errors.push(e.to_string());
        }
        ensure!(
            rollback_errors.is_empty(),
            "Installation failed: {error:#}. Configuration restoration needs attention: {}",
            rollback_errors.join("; ")
        );
        return Err(error.context("installation failed; previous configuration restored"));
    }
    println!(
        "Installed Clippy {} for this Windows user.",
        env!("CARGO_PKG_VERSION")
    );
    println!("Open a new PowerShell tab in Windows Terminal and use ssh normally.");
    println!("Clippy starts at sign-in. Controls: clippy background status|pause|resume|stop");
    println!("Native bypass: ssh.exe ...   Uninstall integration: clippy uninstall");
    println!(
        "Existing aliases/functions named ssh or clippy take precedence. Explicit PowerShell and organization policies were preserved."
    );
    Ok(())
}
pub fn uninstall() -> Result<()> {
    let root = root()?;
    let manifest_path = root.join("installation.json");
    let bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("Clippy integration is already removed.");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    let documents = documents()?;
    let expected = [
        documents.join("WindowsPowerShell").join("profile.ps1"),
        documents.join("PowerShell").join("profile.ps1"),
    ];
    ensure!(
        manifest.profiles.iter().all(|p| expected.contains(p)),
        "unexpected profile paths in installation record"
    );
    ensure!(
        manifest
            .client
            .canonicalize()?
            .starts_with(root.canonicalize()?),
        "unexpected installed client path"
    );
    let edits = manifest
        .profiles
        .iter()
        .map(|p| edit_profile(p, None))
        .collect::<Result<Vec<_>>>()?;
    for (path, (old, new)) in manifest.profiles.iter().zip(edits) {
        if old != new {
            atomic_write(path, &new)?;
        }
    }
    if startup_value()?.as_deref() == Some(&manifest.startup) {
        set_startup(None)?;
    }
    policy::restore(&manifest.policy_changes)?;
    let _ = Command::new(&manifest.client)
        .args(["background", "stop"])
        .creation_flags(CREATE_NO_WINDOW.0)
        .status();
    fs::remove_file(manifest_path)?;
    println!(
        "Removed Clippy's startup entry and PowerShell integration. Open new tabs to reload your profiles."
    );
    println!("Existing SSH connections, installed binaries, and uploaded files are retained.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_preserve_unicode_encoding() {
        for encoding in [Encoding::Utf8, Encoding::Utf8Bom, Encoding::Utf16Le] {
            let original = encode("# 日本語\r\nWrite-Output 'hello'\r\n", encoding);
            let (text, detected) = decode(&original).unwrap();
            let changed = profile_block(&text, Some("C:\\Clippy\\ssh.ps1")).unwrap();
            let restored = profile_block(&changed, None).unwrap();
            assert_eq!(encode(&restored, detected), original);
        }
    }
    #[test]
    fn atomic_profile_update_keeps_other_functions() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile.ps1");
        atomic_write(&profile, b"function mine {}\n").unwrap();
        let (_, bytes) = edit_profile(&profile, Some(Path::new("C:\\Clippy\\ssh.ps1"))).unwrap();
        atomic_write(&profile, &bytes).unwrap();
        let (_, restored) = edit_profile(&profile, None).unwrap();
        assert_eq!(restored, b"function mine {}\n");
    }
}

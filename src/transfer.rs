use crate::protocol::{CHUNK, FileSpec, MAX_FILES};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name != "." && name != ".." && name.len() <= 255,
        "invalid filename"
    );
    ensure!(
        !name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\'),
        "unsupported filename: control characters or separators"
    );
    Ok(())
}
pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "refusing symlink directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            fs::metadata(path)?.uid() == unsafe { libc::geteuid() },
            "directory belongs to another user"
        );
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub fn upload_root(home: &Path) -> Result<PathBuf> {
    let home = home.canonicalize()?;
    let mut current = home;
    for part in [".local", "share", "clippy", "uploads"] {
        current.push(part);
        if let Ok(m) = fs::symlink_metadata(&current) {
            ensure!(
                !m.file_type().is_symlink(),
                "upload path contains a symlink"
            );
        }
        fs::create_dir_all(&current)?;
    }
    private_dir(&current)?;
    Ok(current)
}
struct PendingFile {
    file: File,
    spec: FileSpec,
    count: u64,
    hash: Sha256,
    verified: bool,
}
pub struct Upload {
    pub id: String,
    stage: PathBuf,
    final_dir: PathBuf,
    files: Vec<PendingFile>,
    committed: bool,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    schema: u16,
    id: String,
    created: u64,
    names: Vec<String>,
}
impl Upload {
    pub fn begin(root: &Path, id: &str, specs: Vec<FileSpec>) -> Result<Self> {
        ensure!(
            uuid::Uuid::parse_str(id).is_ok() && id.len() == 36,
            "invalid upload ID"
        );
        ensure!(
            !specs.is_empty() && specs.len() <= MAX_FILES,
            "invalid file count"
        );
        for s in &specs {
            validate_name(&s.name)?;
        }
        let stage = root.join(format!(".partial-{id}"));
        let final_dir = root.join(id);
        ensure!(!final_dir.exists(), "upload already exists");
        fs::create_dir(&stage).context("create staging directory")?;
        let mut upload = Self {
            id: id.into(),
            stage,
            final_dir,
            files: vec![],
            committed: false,
        };
        private_dir(&upload.stage)?;
        for (i, spec) in specs.into_iter().enumerate() {
            let dir = upload.stage.join(i.to_string());
            private_dir(&dir)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let file = options.open(dir.join(&spec.name))?;
            upload.files.push(PendingFile {
                file,
                spec,
                count: 0,
                hash: Sha256::new(),
                verified: false,
            });
        }
        Ok(upload)
    }
    pub fn chunk(&mut self, index: usize, offset: u64, data: &[u8]) -> Result<()> {
        ensure!(data.len() <= CHUNK, "oversized file chunk");
        let f = self.files.get_mut(index).context("invalid file index")?;
        ensure!(
            !f.verified && offset == f.count,
            "out-of-order or duplicate file chunk"
        );
        let next = f
            .count
            .checked_add(data.len() as u64)
            .context("file size overflow")?;
        ensure!(next <= f.spec.size, "file exceeded declared size");
        f.file.write_all(data)?;
        f.hash.update(data);
        f.count = next;
        Ok(())
    }
    pub fn finish(&mut self, index: usize, expected: [u8; 32]) -> Result<()> {
        let f = self.files.get_mut(index).context("invalid file index")?;
        ensure!(!f.verified && f.count == f.spec.size, "incomplete file");
        let actual: [u8; 32] = f.hash.clone().finalize().into();
        ensure!(actual == expected, "SHA-256 mismatch");
        f.file.sync_all()?;
        f.verified = true;
        Ok(())
    }
    pub fn commit(mut self) -> Result<Vec<String>> {
        ensure!(
            self.files.iter().all(|f| f.verified),
            "batch contains unverified files"
        );
        let manifest = Manifest {
            schema: 1,
            id: self.id.clone(),
            created: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            names: self.files.iter().map(|f| f.spec.name.clone()).collect(),
        };
        let names = manifest.names.clone();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.stage.join(".clippy-manifest.json"))?;
        serde_json::to_writer(&mut file, &manifest)?;
        file.sync_all()?;
        drop(file);
        // Close every file before renaming (also required on Windows test hosts).
        self.files.clear();
        fs::rename(&self.stage, &self.final_dir)?;
        self.committed = true;
        names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                self.final_dir
                    .join(i.to_string())
                    .join(n)
                    .into_os_string()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("non-UTF-8 remote home is unsupported"))
            })
            .collect()
    }
}
impl Drop for Upload {
    fn drop(&mut self) {
        if !self.committed {
            self.files.clear();
            let _ = fs::remove_dir_all(&self.stage);
        }
    }
}

pub fn clean(root: &Path, older_than: u64) -> Result<usize> {
    ensure!(older_than > 0, "cleanup age must be positive");
    let cutoff = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .saturating_sub(older_than);
    let mut count = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if uuid::Uuid::parse_str(&name).is_err() || !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let mp = path.join(".clippy-manifest.json");
        if fs::symlink_metadata(&mp)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(m) = serde_json::from_slice::<Manifest>(&fs::read(&mp)?) else {
            continue;
        };
        if m.schema == 1 && m.id == name && m.created < cutoff {
            ensure!(
                path.canonicalize()?.parent() == Some(root.canonicalize()?.as_path()),
                "cleanup escaped upload root"
            );
            fs::remove_dir_all(path)?;
            count += 1;
        }
    }
    Ok(count)
}
pub fn duration_seconds(s: &str) -> Result<u64> {
    let (digits, mult) = match s.as_bytes().last() {
        Some(b'd') => (&s[..s.len() - 1], 86400),
        Some(b'h') => (&s[..s.len() - 1], 3600),
        Some(b'm') => (&s[..s.len() - 1], 60),
        Some(b's') => (&s[..s.len() - 1], 1),
        _ => bail!("use an age such as 30d, 12h, 15m, or 60s"),
    };
    let n = digits
        .parse::<u64>()?
        .checked_mul(mult)
        .context("duration overflow")?;
    ensure!(n > 0, "duration must be positive");
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> Vec<FileSpec> {
        vec![FileSpec {
            name: "a b's.png".into(),
            size: 3,
        }]
    }
    #[test]
    fn verified_atomic_batch() {
        let d = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let mut u = Upload::begin(d.path(), &id, spec()).unwrap();
        u.chunk(0, 0, b"abc").unwrap();
        assert!(!d.path().join(&id).exists());
        u.finish(0, Sha256::digest(b"abc").into()).unwrap();
        let paths = u.commit().unwrap();
        assert_eq!(fs::read(&paths[0]).unwrap(), b"abc");
    }
    #[test]
    fn corrupt_cancelled_and_traversal_batches_leave_no_files() {
        let d = tempfile::tempdir().unwrap();
        {
            let mut u = Upload::begin(d.path(), &uuid::Uuid::new_v4().to_string(), spec()).unwrap();
            assert!(u.chunk(0, 1, b"abc").is_err());
            u.chunk(0, 0, b"abc").unwrap();
            assert!(u.finish(0, [0; 32]).is_err());
        }
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0);
        for n in ["../bad", "a/b", "a\\b", "a\nb", "..", ""] {
            assert!(validate_name(n).is_err());
        }
    }
    #[test]
    fn same_names_and_empty_files() {
        let d = tempfile::tempdir().unwrap();
        let mut u = Upload::begin(
            d.path(),
            &uuid::Uuid::new_v4().to_string(),
            vec![
                FileSpec {
                    name: "same".into(),
                    size: 0
                };
                2
            ],
        )
        .unwrap();
        for i in 0..2 {
            u.finish(i, Sha256::digest([]).into()).unwrap();
        }
        let p = u.commit().unwrap();
        assert_ne!(p[0], p[1]);
    }
    #[test]
    fn unverified_commit_is_removed() {
        let d = tempfile::tempdir().unwrap();
        let u = Upload::begin(d.path(), &uuid::Uuid::new_v4().to_string(), spec()).unwrap();
        assert!(u.commit().is_err());
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0);
    }
    #[test]
    fn cleanup_ignores_unmanaged_directories() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("personal")).unwrap();
        assert_eq!(clean(d.path(), 1).unwrap(), 0);
        assert!(d.path().join("personal").exists());
    }
}

use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use crate::{error, Result};

pub(crate) const BUNDLE_CAP: usize = 1024 * 1024;
pub(crate) const EVIDENCE_CAP: usize = 32 * 1024 * 1024;

pub(crate) fn check_name(value: &str) -> Result<()> {
    if (1..=64).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        Ok(())
    } else {
        Err(error("device name must be 1–64 letters, digits, underscores or hyphens"))
    }
}

pub(crate) fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    random_into(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn random_into(bytes: &mut [u8]) -> Result<()> {
    getrandom::fill(bytes).map_err(|_| error("operating system RNG unavailable"))
}

pub(crate) fn read(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected a regular file"));
    }
    read_bounded(file, limit)
}

fn read_bounded(file: File, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "input exceeds size limit"));
    }
    Ok(bytes)
}

pub(crate) struct PrivateDir(PathBuf);

impl PrivateDir {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        DirBuilder::new().recursive(true).mode(0o700).create(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(error("development storage must be a regular directory with mode 0700"));
        }
        Ok(Self(path.to_owned()))
    }

    pub(crate) fn subdir(&self, name: &str) -> Result<Self> {
        Self::open(&self.0.join(name))
    }

    pub(crate) fn lock(&self) -> Result<File> {
        let lock = OpenOptions::new().write(true).create(true).truncate(false).mode(0o600)
            .open(self.0.join(".lock"))?;
        lock.try_lock().map_err(|_| error("another development process holds this directory lock"))?;
        Ok(lock)
    }

    pub(crate) fn read(&self, name: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        let path = self.0.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() && meta.permissions().mode() & 0o777 == 0o600 => {
                Ok(Some(read(&path, limit)?))
            }
            Ok(_) => Err(error("development storage entry must be a regular file with mode 0600")),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        write_atomic(&self.0.join(name), bytes)
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".galaxy-{}", hex::encode(random::<16>()?)));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    let result = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

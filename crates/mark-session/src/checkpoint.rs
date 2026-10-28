//! Private, atomic local checkpoints. A kernel lock covers every store operation.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) const MAX_CHECKPOINT_BYTES: usize = 4 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

pub(crate) struct LockedDirectory {
    path: PathBuf,
    _lock: File,
}

impl LockedDirectory {
    #[cfg(unix)]
    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        use std::os::{
            fd::AsRawFd,
            unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        };

        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut current = PathBuf::new();
        for part in path.components() {
            match part {
                Component::RootDir | Component::Normal(_) => current.push(part.as_os_str()),
                Component::CurDir => continue,
                _ => {
                    return Err(invalid(
                        "invalid_store_path",
                        "store path must be normalized",
                    ));
                }
            }
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(invalid(
                        "unsafe_store_path",
                        "store ancestors must be directories, not symlinks",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let mut builder = fs::DirBuilder::new();
                    builder.mode(0o700);
                    match builder.create(&current) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    let metadata = fs::symlink_metadata(&current)?;
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(invalid(
                            "unsafe_store_path",
                            "store ancestor changed during creation",
                        ));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe_store_permissions: review directory must belong to this user and be private",
            ));
        }
        let lock_path = path.join(".lock");
        reject_symlink(&lock_path)?;
        let mut options = private_options();
        let lock = options
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        verify_file(&lock)?;
        // flock is supported by both target platforms and releases on close or crash.
        unsafe extern "C" {
            fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
        }
        loop {
            // SAFETY: the live File owns this descriptor; LOCK_EX has no pointer arguments.
            if unsafe { flock(lock.as_raw_fd(), 2) } == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        Ok(Self { path, _lock: lock })
    }

    #[cfg(not(unix))]
    pub(crate) fn acquire(_path: &Path) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "review checkpoints require Unix file locking",
        ))
    }

    pub(crate) fn ids(&self) -> io::Result<Vec<String>> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(id) = name.strip_suffix(".json") {
                // Older checkpoints share this directory but use a different
                // filename format. Discover only this store's records; direct
                // reads and writes still reject invalid IDs.
                if validate_id(id).is_ok() {
                    ids.push(id.to_owned());
                }
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub(crate) fn read(&self, id: &str) -> io::Result<Vec<u8>> {
        validate_id(id)?;
        let path = self.path.join(format!("{id}.json"));
        reject_symlink(&path)?;
        let file = private_options().read(true).open(path)?;
        verify_file(&file)?;
        let mut bytes = Vec::new();
        file.take(MAX_CHECKPOINT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(invalid("checkpoint_limit", "checkpoint exceeds byte limit"));
        }
        Ok(bytes)
    }

    pub(crate) fn write(&self, id: &str, bytes: &[u8]) -> io::Result<()> {
        validate_id(id)?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(invalid(
                "checkpoint_limit",
                "review exceeds checkpoint byte limit",
            ));
        }
        let destination = self.path.join(format!("{id}.json"));
        reject_symlink(&destination)?;
        let temporary = self.path.join(format!(
            ".{id}.{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = private_options()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            reject_symlink(&destination)?;
            fs::rename(&temporary, &destination)?;
            File::open(&self.path)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

pub(crate) fn validate_id(id: &str) -> io::Result<()> {
    if id.len() != 24
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid(
            "invalid_review_id",
            "review ID must contain 24 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(invalid(
            "unsafe_checkpoint",
            "checkpoint or lock is not a regular file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "linux")]
        const NO_FOLLOW: i32 = 0x20000;
        #[cfg(not(target_os = "linux"))]
        const NO_FOLLOW: i32 = 0x100;
        options.mode(0o600).custom_flags(NO_FOLLOW);
    }
    options
}

fn verify_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("unsafe_checkpoint", "expected a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe_checkpoint_permissions: checkpoint must belong to this user and be private",
            ));
        }
    }
    Ok(())
}

pub(crate) fn invalid(code: &str, message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{code}: {message}"))
}

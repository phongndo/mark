use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Replaces `path` atomically with `contents`, retaining existing permissions.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let destination = write_destination(path)?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let permissions = fs::metadata(&destination)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    temporary.write_all(contents)?;
    temporary.as_file_mut().flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&destination)
        .map_err(|error| error.error)?;
    sync_parent_directory(parent)
}

/// Resolves the final symlink component without requiring its target to exist.
fn write_destination(path: &Path) -> io::Result<PathBuf> {
    const MAX_SYMLINKS: usize = 40;

    let mut destination = path.to_path_buf();
    let mut followed = 0;
    loop {
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if followed == MAX_SYMLINKS {
                    return Err(io::Error::other("too many levels of symbolic links"));
                }
                let target = fs::read_link(&destination)?;
                destination = if target.is_absolute() {
                    target
                } else {
                    destination
                        .parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .unwrap_or_else(|| Path::new("."))
                        .join(target)
                };
                followed += 1;
            }
            Ok(_) => return Ok(destination),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(destination),
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    match fs::File::open(parent)?.sync_all() {
        Ok(()) => Ok(()),
        // Some network and pseudo filesystems do not support directory fsync.
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn atomic_write_replaces_complete_contents() {
        let test_dir = test_dir("mark-core-atomic-write-test");
        let path = test_dir.join("config.toml");
        fs::write(&path, "old").expect("old contents should be written");

        atomic_write(&path, b"new contents\n").expect("atomic write should succeed");

        assert_eq!(
            fs::read_to_string(&path).expect("new contents should be readable"),
            "new contents\n"
        );
        assert_eq!(
            fs::read_dir(&test_dir)
                .expect("directory should be readable")
                .count(),
            1
        );
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_updates_a_symlink_target_without_replacing_the_link() {
        let test_dir = test_dir("mark-core-atomic-symlink-test");
        let target = test_dir.join("target.toml");
        let link = test_dir.join("config.toml");
        fs::write(&target, "old").expect("target should be written");
        std::os::unix::fs::symlink(&target, &link).expect("symlink should be created");

        atomic_write(&link, b"new").expect("atomic write should succeed");

        assert!(
            fs::symlink_metadata(&link)
                .expect("link metadata should exist")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(target).expect("target should be readable"),
            "new"
        );
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_creates_a_dangling_symlink_target_without_replacing_the_link() {
        let test_dir = test_dir("mark-core-atomic-dangling-symlink-test");
        let target = test_dir.join("target.toml");
        let link = test_dir.join("config.toml");
        std::os::unix::fs::symlink("target.toml", &link).expect("symlink should be created");

        atomic_write(&link, b"new").expect("atomic write should succeed");

        assert!(
            fs::symlink_metadata(&link)
                .expect("link metadata should exist")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(target).expect("target should be readable"),
            "new"
        );
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    fn test_dir(prefix: &str) -> PathBuf {
        let test_dir = env::temp_dir().join(format!(
            "{prefix}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        test_dir
    }
}

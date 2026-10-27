use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub(super) fn write_package(
    output: &Path,
    files: &BTreeMap<PathBuf, Vec<u8>>,
) -> io::Result<PathBuf> {
    let output = absolute(output)?;
    ensure_directory(&output)?;
    let sealed = output.join(".mark-integration.json").try_exists()?;
    // Check the entire existing tree before creating any package file. A changed
    // or missing sealed file is drift, and unknown files are left untouched.
    inspect(&output, &output, files)?;
    for (name, expected) in files {
        let path = output.join(name);
        match read_regular(&path) {
            Ok(bytes) if bytes == *expected => {}
            Ok(_) => return Err(drift(&path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !sealed => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(drift(&path)),
            Err(error) => return Err(error),
        }
    }
    // The marker is written last so an interrupted first materialization can be
    // resumed, while intentional edits to a completed package are preserved.
    for (name, bytes) in files
        .iter()
        .filter(|(path, _)| path.as_path() != Path::new(".mark-integration.json"))
    {
        write_missing(&output.join(name), bytes)?;
    }
    write_missing(
        &output.join(".mark-integration.json"),
        &files[Path::new(".mark-integration.json")],
    )?;
    inspect(&output, &output, files)?;
    Ok(output)
}

fn inspect(root: &Path, directory: &Path, expected: &BTreeMap<PathBuf, Vec<u8>>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(unsafe_path(&path));
        }
        if kind.is_dir() {
            inspect(root, &path, expected)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(root).expect("package descendant");
            let bytes = expected.get(relative).ok_or_else(|| drift(&path))?;
            if read_regular(&path)? != *bytes {
                return Err(drift(&path));
            }
        } else {
            return Err(unsafe_path(&path));
        }
    }
    Ok(())
}

fn write_missing(path: &Path, bytes: &[u8]) -> io::Result<()> {
    match read_regular(path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => return Err(drift(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = path.parent().expect("package file parent");
    ensure_directory(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            if read_regular(path)? == bytes {
                Ok(())
            } else {
                Err(drift(path))
            }
        }
        Err(error) => Err(error.error),
    }
}

fn read_regular(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(unsafe_path(path));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "linux")]
        const NO_FOLLOW: i32 = 0x20000;
        #[cfg(not(target_os = "linux"))]
        const NO_FOLLOW: i32 = 0x100;
        options.custom_flags(NO_FOLLOW);
    }
    let mut bytes = Vec::new();
    options
        .open(path)?
        .take(16 * 1024 * 1024)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn absolute(path: &Path) -> io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            _ => return Err(unsafe_path(path.as_path())),
        }
    }
    Ok(normalized)
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(unsafe_path(&current)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                match builder.create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                let metadata = fs::symlink_metadata(&current)?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(unsafe_path(&current));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn unsafe_path(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "unsafe_integration_path: expected ordinary directories/files without symlinks: {}",
            path.display()
        ),
    )
}
fn drift(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "integration_modified: preserving {}. Export to a new empty directory to get a clean package.",
            path.display()
        ),
    )
}

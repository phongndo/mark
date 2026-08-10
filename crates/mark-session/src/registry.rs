use std::{
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::protocol::{PROTOCOL_VERSION, SessionListing, SessionRecord};

const RECORD_FILE: &str = "session.json";
const SOCKET_FILE: &str = "session.sock";
const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct Registry {
    base: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionError {
    NotFound,
    Ambiguous(Vec<String>),
    InvalidRepository(String),
    Io(String),
}

impl std::fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("no live Mark session matched"),
            Self::Ambiguous(ids) => write!(
                formatter,
                "multiple Mark sessions matched: {}",
                ids.join(", ")
            ),
            Self::InvalidRepository(path) => {
                write!(formatter, "repository does not exist: {path}")
            }
            Self::Io(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for SelectionError {}

impl Registry {
    pub fn discover() -> io::Result<Self> {
        let base = runtime_base_directory()?;
        Ok(Self { base })
    }

    pub fn at(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    pub fn session_dir(&self, session_id: &str) -> io::Result<PathBuf> {
        validate_session_id(session_id)?;
        Ok(self.base.join(session_id))
    }

    pub fn socket_path(&self, session_id: &str) -> io::Result<PathBuf> {
        Ok(self.session_dir(session_id)?.join(SOCKET_FILE))
    }

    pub fn record_path(&self, session_id: &str) -> io::Result<PathBuf> {
        Ok(self.session_dir(session_id)?.join(RECORD_FILE))
    }

    pub fn prepare_session(&self, session_id: &str) -> io::Result<PathBuf> {
        ensure_private_directory(&self.base)?;
        let session_dir = self.session_dir(session_id)?;
        ensure_private_directory(&session_dir)?;
        Ok(session_dir)
    }

    pub fn write_record(&self, record: &SessionRecord) -> io::Result<()> {
        validate_record(self, record)?;
        let session_dir = self.prepare_session(&record.session_id)?;
        let record_path = session_dir.join(RECORD_FILE);
        let temp_path =
            session_dir.join(format!(".session-{}-{}.tmp", std::process::id(), nonce()));
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "session record is too large",
            ));
        }

        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp_path, &record_path)?;
        set_private_file_permissions(&record_path)?;
        Ok(())
    }

    pub fn list(&self) -> io::Result<Vec<SessionListing>> {
        ensure_private_directory(&self.base)?;
        let mut sessions = Vec::new();
        for entry in fs::read_dir(&self.base)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let session_id = entry.file_name().to_string_lossy().into_owned();
            if validate_session_id(&session_id).is_err() {
                continue;
            }
            let record = match self.read_record(&session_id) {
                Ok(record) => record,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    // The server creates its private directory and listener before
                    // atomically publishing the record. A missing record can therefore
                    // be an in-progress registration and is not proof of staleness.
                    continue;
                }
                Err(_) => {
                    let _ = self.remove_session(&session_id);
                    continue;
                }
            };
            let identity_matches = process_identity(record.process_id)
                .is_none_or(|identity| identity == record.process_identity);
            if !identity_matches {
                let _ = self.remove_session(&session_id);
                continue;
            }
            match endpoint_status(Path::new(&record.endpoint)) {
                EndpointStatus::Responsive => sessions.push(SessionListing {
                    record,
                    responsive: true,
                }),
                EndpointStatus::Stale => {
                    let _ = self.remove_session(&session_id);
                }
                EndpointStatus::Indeterminate => sessions.push(SessionListing {
                    record,
                    responsive: false,
                }),
            }
        }
        sessions.sort_by(|left, right| left.record.session_id.cmp(&right.record.session_id));
        Ok(sessions)
    }

    pub fn read_record(&self, session_id: &str) -> io::Result<SessionRecord> {
        let session_dir = self.session_dir(session_id)?;
        verify_private_directory(&session_dir)?;
        let record_path = session_dir.join(RECORD_FILE);
        verify_private_file(&record_path)?;
        let mut file = fs::File::open(&record_path)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take((MAX_RECORD_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "session record is too large",
            ));
        }
        let record: SessionRecord = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        validate_record(self, &record)?;
        if record.session_id != session_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "session record ID does not match its directory",
            ));
        }
        Ok(record)
    }

    pub fn select(
        &self,
        session_id: Option<&str>,
        repository: Option<&Path>,
    ) -> Result<SessionListing, SelectionError> {
        let sessions = self
            .list()
            .map_err(|error| SelectionError::Io(error.to_string()))?;
        if let Some(session_id) = session_id {
            return sessions
                .into_iter()
                .find(|session| session.record.session_id == session_id)
                .ok_or(SelectionError::NotFound);
        }

        let canonical_repository = repository
            .map(|path| {
                fs::canonicalize(path)
                    .map_err(|_| SelectionError::InvalidRepository(path.display().to_string()))
            })
            .transpose()?;
        let mut matches = sessions
            .into_iter()
            .filter(|session| {
                canonical_repository.as_ref().is_none_or(|repository| {
                    Path::new(&session.record.repository) == repository.as_path()
                })
            })
            .collect::<Vec<_>>();
        match matches.len() {
            0 => Err(SelectionError::NotFound),
            1 => Ok(matches.remove(0)),
            _ => Err(SelectionError::Ambiguous(
                matches
                    .into_iter()
                    .map(|session| session.record.session_id)
                    .collect(),
            )),
        }
    }

    pub fn remove_session(&self, session_id: &str) -> io::Result<()> {
        let session_dir = self.session_dir(session_id)?;
        if !session_dir.exists() {
            return Ok(());
        }
        verify_private_directory(&session_dir)?;
        for name in [SOCKET_FILE, RECORD_FILE] {
            let path = session_dir.join(name);
            if path
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "refusing to remove a symlinked session artifact",
                ));
            }
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        match fs::remove_dir(session_dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

pub fn new_session_id() -> String {
    let mixed = (nonce() as u64)
        ^ (u64::from(std::process::id()).rotate_left(17))
        ^ process_nonce().rotate_left(37);
    format!("{:012x}", mixed & 0x0000_ffff_ffff_ffff)
}

pub fn current_process_identity() -> String {
    if let Some(identity) = process_identity(std::process::id()) {
        return identity;
    }
    let executable = env::current_exe()
        .ok()
        .and_then(|path| fs::canonicalize(path).ok())
        .unwrap_or_default();
    format!("pid:{}:{}", std::process::id(), executable.display())
}

fn process_identity(process_id: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{process_id}/stat")).ok()?;
        let after_name = stat.rsplit_once(") ").map(|(_, rest)| rest)?;
        let start_time = after_name.split_whitespace().nth(19)?;
        return Some(format!("linux:{process_id}:{start_time}"));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = process_id;
        None
    }
}

fn validate_record(registry: &Registry, record: &SessionRecord) -> io::Result<()> {
    validate_session_id(&record.session_id)?;
    if record.protocol != PROTOCOL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported protocol in session record",
        ));
    }
    let expected = registry.socket_path(&record.session_id)?;
    if Path::new(&record.endpoint) != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "session endpoint is outside its private directory",
        ));
    }
    Ok(())
}

fn validate_session_id(session_id: &str) -> io::Result<()> {
    if session_id.is_empty()
        || session_id.len() > 96
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session ID",
        ));
    }
    Ok(())
}

fn runtime_base_directory() -> io::Result<PathBuf> {
    if let Some(runtime) = env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(runtime).join("mark"));
    }
    Ok(env::temp_dir().join(format!("mark-{}", current_uid())))
}

fn nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn process_nonce() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[cfg(unix)]
fn current_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

fn ensure_private_directory(path: &Path) -> io::Result<()> {
    match path.symlink_metadata() {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "session registry is not a private directory",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
        }
        Err(error) => return Err(error),
    }
    set_private_directory_permissions(path)?;
    verify_private_directory(path)
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn verify_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "session directory is not owned and private",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_directory(path: &Path) -> io::Result<()> {
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "session registry is not a private directory",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_private_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "session record is not owned and private",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_file(path: &Path) -> io::Result<()> {
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid session record",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointStatus {
    Responsive,
    Stale,
    Indeterminate,
}

#[cfg(unix)]
fn endpoint_status(path: &Path) -> EndpointStatus {
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => EndpointStatus::Responsive,
        Err(error) => classify_endpoint_error(error.kind()),
    }
}

#[cfg(not(unix))]
fn endpoint_status(_path: &Path) -> EndpointStatus {
    classify_endpoint_error(io::ErrorKind::Unsupported)
}

fn classify_endpoint_error(kind: io::ErrorKind) -> EndpointStatus {
    match kind {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => EndpointStatus::Stale,
        _ => EndpointStatus::Indeterminate,
    }
}

#[cfg(unix)]
pub(crate) fn private_socket(path: &Path) -> io::Result<()> {
    set_private_file_permissions(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_errors_only_mark_definitive_socket_failures_stale() {
        assert_eq!(
            classify_endpoint_error(io::ErrorKind::NotFound),
            EndpointStatus::Stale
        );
        assert_eq!(
            classify_endpoint_error(io::ErrorKind::ConnectionRefused),
            EndpointStatus::Stale
        );
        assert_eq!(
            classify_endpoint_error(io::ErrorKind::PermissionDenied),
            EndpointStatus::Indeterminate
        );
        assert_eq!(
            classify_endpoint_error(io::ErrorKind::WouldBlock),
            EndpointStatus::Indeterminate
        );
    }
}

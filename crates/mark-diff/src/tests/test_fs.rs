pub use std::fs::*;

pub fn remove_dir_all(path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
    const MAX_ATTEMPTS: u32 = 5;

    let path = path.as_ref();
    for attempt in 0..MAX_ATTEMPTS {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            // CI has observed a transient ENOTEMPTY while removing a freshly-written Git
            // repository on macOS. Retry only that condition and surface persistent errors.
            Err(error)
                if error.kind() == std::io::ErrorKind::DirectoryNotEmpty
                    && attempt + 1 < MAX_ATTEMPTS =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10u64 << attempt));
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("cleanup retry loop must return")
}

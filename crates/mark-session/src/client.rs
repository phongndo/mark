use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(unix)]
use std::io::{Read, Write};

#[cfg(any(unix, test))]
use crate::protocol::METHOD_RELOAD;
#[cfg(unix)]
use crate::protocol::{MAX_REQUEST_FRAME_BYTES, MAX_RESPONSE_FRAME_BYTES, PROTOCOL_VERSION};
use crate::protocol::{Request, Response};

#[derive(Debug, Clone)]
pub struct Client {
    endpoint: PathBuf,
    timeout: Duration,
    #[cfg(any(unix, test))]
    reload_timeout: Option<Duration>,
}

impl Client {
    pub fn new(endpoint: impl Into<PathBuf>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout: Duration::from_secs(30),
            #[cfg(any(unix, test))]
            reload_timeout: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        #[cfg(any(unix, test))]
        {
            self.reload_timeout = Some(timeout);
        }
        self
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    #[cfg(any(unix, test))]
    fn read_timeout(&self, method: &str) -> Option<Duration> {
        if method == METHOD_RELOAD {
            self.reload_timeout
        } else {
            Some(self.timeout)
        }
    }

    #[cfg(unix)]
    pub fn request(&self, request: &Request) -> io::Result<Response> {
        use std::os::unix::net::UnixStream;

        let mut bytes = serde_json::to_vec(request)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() > MAX_REQUEST_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "request exceeds the configured frame limit",
            ));
        }
        bytes.push(b'\n');

        let mut stream = UnixStream::connect(&self.endpoint)?;
        stream.set_read_timeout(self.read_timeout(&request.method))?;
        stream.set_write_timeout(Some(self.timeout))?;
        stream.write_all(&bytes)?;
        stream.shutdown(std::net::Shutdown::Write)?;

        let mut response_bytes = Vec::new();
        Read::by_ref(&mut stream)
            .take((MAX_RESPONSE_FRAME_BYTES + 2) as u64)
            .read_to_end(&mut response_bytes)?;
        if response_bytes.len() > MAX_RESPONSE_FRAME_BYTES + 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response exceeds the configured frame limit",
            ));
        }
        if response_bytes.last() == Some(&b'\n') {
            response_bytes.pop();
        }
        let response: Response = serde_json::from_slice(&response_bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if response.protocol != PROTOCOL_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "session returned an unsupported protocol",
            ));
        }
        if response.id != request.id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "session response ID does not match the request",
            ));
        }
        Ok(response)
    }

    #[cfg(not(unix))]
    pub fn request(&self, _request: &Request) -> io::Result<Response> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "live sessions are not supported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reloads_wait_for_completion_unless_a_timeout_is_explicit() {
        let client = Client::new("endpoint");
        assert_eq!(client.read_timeout(METHOD_RELOAD), None);
        assert_eq!(
            client.read_timeout("session.get"),
            Some(Duration::from_secs(30))
        );

        let client = client.with_timeout(Duration::from_secs(2));
        assert_eq!(
            client.read_timeout(METHOD_RELOAD),
            Some(Duration::from_secs(2))
        );
    }
}

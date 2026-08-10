#[cfg(unix)]
use std::{io, path::Path, sync::Arc, time::Duration};

use tokio::sync::{mpsc, oneshot};

#[cfg(unix)]
use crate::{
    protocol::{
        MAX_REQUEST_FRAME_BYTES, MAX_REQUEST_ID_BYTES, MAX_RESPONSE_FRAME_BYTES, PROTOCOL_VERSION,
        ProtocolError,
    },
    registry::private_socket,
    transport::{read_frame, write_frame},
};
use crate::{
    protocol::{Request, Response, SessionRecord},
    registry::Registry,
};

#[cfg(unix)]
const MAX_CONNECTED_CLIENTS: usize = 32;
#[cfg(unix)]
const CLIENT_FRAME_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(unix)]
const CLIENT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct SessionCommand {
    pub request: Request,
    pub reply: oneshot::Sender<Response>,
}

#[cfg(unix)]
#[derive(Debug)]
pub struct ServerHandle {
    task: tokio::task::JoinHandle<()>,
    registry: Registry,
    session_id: String,
}

#[cfg(unix)]
impl ServerHandle {
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub fn abort(&self) {
        self.task.abort();
    }
}

#[cfg(unix)]
impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.task.abort();
        let _ = self.registry.remove_session(&self.session_id);
    }
}

#[cfg(not(unix))]
#[derive(Debug)]
pub struct ServerHandle;

#[cfg(not(unix))]
impl ServerHandle {
    pub fn is_finished(&self) -> bool {
        true
    }

    pub fn abort(&self) {}
}

#[cfg(unix)]
pub fn spawn_server(
    registry: Registry,
    record: SessionRecord,
    commands: mpsc::Sender<SessionCommand>,
) -> (ServerHandle, oneshot::Receiver<Result<(), String>>) {
    let (startup_tx, startup_rx) = oneshot::channel();
    let handle_registry = registry.clone();
    let handle_session_id = record.session_id.clone();
    let task = tokio::spawn(async move {
        let result = run_server(registry, record, commands, startup_tx).await;
        if let Err(error) = result {
            // Startup errors are sent over `startup_tx`; errors after startup
            // close the endpoint and are observed by clients and registry scans.
            let _ = error;
        }
    });
    (
        ServerHandle {
            task,
            registry: handle_registry,
            session_id: handle_session_id,
        },
        startup_rx,
    )
}

#[cfg(not(unix))]
pub fn spawn_server(
    _registry: Registry,
    _record: SessionRecord,
    _commands: mpsc::Sender<SessionCommand>,
) -> (ServerHandle, oneshot::Receiver<Result<(), String>>) {
    let (startup_tx, startup_rx) = oneshot::channel();
    let error = "live sessions are not supported on this platform".to_owned();
    let _ = startup_tx.send(Err(error));
    (ServerHandle, startup_rx)
}

#[cfg(unix)]
async fn run_server(
    registry: Registry,
    record: SessionRecord,
    commands: mpsc::Sender<SessionCommand>,
    startup_tx: oneshot::Sender<Result<(), String>>,
) -> io::Result<()> {
    let session_dir = match registry.prepare_session(&record.session_id) {
        Ok(path) => path,
        Err(error) => {
            let _ = startup_tx.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let _registration = Registration {
        registry: registry.clone(),
        session_id: record.session_id.clone(),
        _session_dir: session_dir,
    };
    let socket_path = Path::new(&record.endpoint);
    if socket_path != registry.socket_path(&record.session_id)? {
        let error = io::Error::new(io::ErrorKind::InvalidInput, "invalid session endpoint");
        let _ = startup_tx.send(Err(error.to_string()));
        return Err(error);
    }
    let _ = std::fs::remove_file(socket_path);
    let listener = match tokio::net::UnixListener::bind(socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = startup_tx.send(Err(error.to_string()));
            return Err(error);
        }
    };
    if let Err(error) = private_socket(socket_path) {
        let _ = startup_tx.send(Err(error.to_string()));
        return Err(error);
    }
    if let Err(error) = registry.write_record(&record) {
        let _ = startup_tx.send(Err(error.to_string()));
        return Err(error);
    }
    let _ = startup_tx.send(Ok(()));

    let clients = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTED_CLIENTS));
    loop {
        let (stream, _) = listener.accept().await?;
        let permit = match Arc::clone(&clients).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => continue,
        };
        let commands = commands.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = handle_connection(stream, commands).await;
        });
    }
}

#[cfg(unix)]
struct Registration {
    registry: Registry,
    session_id: String,
    _session_dir: std::path::PathBuf,
}

#[cfg(unix)]
impl Drop for Registration {
    fn drop(&mut self) {
        let _ = self.registry.remove_session(&self.session_id);
    }
}

#[cfg(unix)]
async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    commands: mpsc::Sender<SessionCommand>,
) -> io::Result<()> {
    if !same_user_peer(&stream)? {
        let response = Response::failure(
            "",
            ProtocolError::new("permission_denied", "session peer belongs to another user"),
        );
        return send_response(&mut stream, response).await;
    }
    let frame = match tokio::time::timeout(
        CLIENT_FRAME_TIMEOUT,
        read_frame(&mut stream, MAX_REQUEST_FRAME_BYTES),
    )
    .await
    {
        Err(_) => {
            return send_response(
                &mut stream,
                Response::failure(
                    "",
                    ProtocolError::new("request_timeout", "request frame timed out"),
                ),
            )
            .await;
        }
        Ok(Ok(Some(frame))) => frame,
        Ok(Ok(None)) => return Ok(()),
        Ok(Err(error)) if error.kind() == io::ErrorKind::InvalidData => {
            let code = if error.to_string().contains("exceeds byte limit") {
                "frame_too_large"
            } else {
                "invalid_frame"
            };
            return send_response(
                &mut stream,
                Response::failure("", ProtocolError::new(code, error.to_string())),
            )
            .await;
        }
        Ok(Err(error)) => return Err(error),
    };
    let request: Request = match serde_json::from_slice(&frame) {
        Ok(request) => request,
        Err(error) => {
            return send_response(
                &mut stream,
                Response::failure(
                    "",
                    ProtocolError::new("invalid_request", format!("invalid JSON request: {error}")),
                ),
            )
            .await;
        }
    };
    if request.protocol != PROTOCOL_VERSION {
        return send_response(
            &mut stream,
            Response::failure(
                request.id,
                ProtocolError::new(
                    "unsupported_protocol",
                    format!("protocol {} is not supported", request.protocol),
                ),
            ),
        )
        .await;
    }
    if request.id.is_empty() || request.id.len() > MAX_REQUEST_ID_BYTES {
        return send_response(
            &mut stream,
            Response::failure(
                request.id,
                ProtocolError::new("invalid_request_id", "request ID is empty or too long"),
            ),
        )
        .await;
    }

    let id = request.id.clone();
    let (reply, response) = oneshot::channel();
    if commands
        .send(SessionCommand { request, reply })
        .await
        .is_err()
    {
        return send_response(
            &mut stream,
            Response::failure(
                id,
                ProtocolError::new("session_unavailable", "session is shutting down"),
            ),
        )
        .await;
    }
    let response = response.await.unwrap_or_else(|_| {
        Response::failure(
            id,
            ProtocolError::new("session_unavailable", "session stopped before replying"),
        )
    });
    send_response(&mut stream, response).await
}

#[cfg(unix)]
async fn send_response(stream: &mut tokio::net::UnixStream, response: Response) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(&response)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if bytes.len() > MAX_RESPONSE_FRAME_BYTES {
        bytes = serde_json::to_vec(&Response::failure(
            response.id,
            ProtocolError::new(
                "response_too_large",
                "response exceeds the configured byte limit",
            ),
        ))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    }
    write_response_frame(stream, &bytes, CLIENT_RESPONSE_TIMEOUT).await
}

#[cfg(unix)]
async fn write_response_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    tokio::time::timeout(timeout, write_frame(writer, frame))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "response frame timed out"))?
}

#[cfg(unix)]
fn same_user_peer(stream: &tokio::net::UnixStream) -> io::Result<bool> {
    let credentials = stream.peer_cred()?;
    Ok(credentials.uid() == rustix::process::getuid().as_raw())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn response_frame_write_times_out_when_the_peer_does_not_read() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (mut writer, _reader) = tokio::io::duplex(1);
            let error =
                write_response_frame(&mut writer, &[b'x'; 8 * 1024], Duration::from_millis(10))
                    .await
                    .unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        });
    }
}

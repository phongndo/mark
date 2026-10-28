use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const VIEW: &str = include_str!("web.html");
const SCRIPT: &str = include_str!("web.js");
const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = mark_session::MAX_REQUEST_FRAME_BYTES;

pub(super) fn launch(review_id: &str) -> io::Result<Value> {
    super::begin(review_id)?;
    let ready = tempfile::NamedTempFile::new()?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["bridge", "web-serve", review_id, "--ready-file"])
        .arg(ready.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let bytes = fs::read(ready.path())?;
        if !bytes.is_empty()
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            if let Some(error) = value["error"].as_str() {
                return Err(io::Error::other(error.to_owned()));
            }
            return Ok(value);
        }
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "local review viewer exited before startup: {status}"
            )));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "local review viewer did not start",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn serve(review_id: &str, ready_file: &Path) -> io::Result<()> {
    let result = serve_inner(review_id, ready_file);
    if let Err(error) = &result {
        let _ = super::write_result(ready_file, &json!({"error":error.to_string()}));
    }
    result
}

fn serve_inner(review_id: &str, ready_file: &Path) -> io::Result<()> {
    super::call("review.get", json!({"review_id":review_id}))?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?.to_string();
    let mut random = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let token = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let prefix = format!("/{token}/");
    let origin = format!("http://{address}");
    super::write_result(
        ready_file,
        &json!({"review_id":review_id,"url":format!("{origin}{prefix}"),"process_id":std::process::id()}),
    )?;
    let mut last_activity = Instant::now();
    while last_activity.elapsed() < Duration::from_secs(3600) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                last_activity = Instant::now();
                // macOS inherits the listener's nonblocking mode. The request
                // parser expects blocking reads bounded by these timeouts.
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(3)))?;
                stream.set_write_timeout(Some(Duration::from_secs(3)))?;
                if let Err(error) = connection(&mut stream, review_id, &prefix, &address, &origin) {
                    let _ = respond(
                        &mut stream,
                        400,
                        "application/json",
                        json!({"ok":false,"error":{"message":error.to_string()}})
                            .to_string()
                            .as_bytes(),
                    );
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25))
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn connection(
    stream: &mut TcpStream,
    review_id: &str,
    prefix: &str,
    address: &str,
    origin: &str,
) -> io::Result<()> {
    let request = read_request(stream)?;
    if request.host != address
        || request
            .origin
            .as_deref()
            .is_some_and(|value| value != origin)
    {
        return respond(
            stream,
            403,
            "text/plain",
            b"This review is available only from its local viewer.",
        );
    }
    let Some(path) = request.path.strip_prefix(prefix) else {
        return respond(stream, 404, "text/plain", b"Review not found");
    };
    if request.method == "GET" && path.is_empty() {
        return respond(
            stream,
            200,
            "text/html; charset=utf-8",
            VIEW.replace("__MARK_SCRIPT__", SCRIPT).as_bytes(),
        );
    }
    if request.method == "GET" && path == "api/review" {
        let result = super::call("review.get", json!({"review_id":review_id})).map(|mut record| {
            record["files"] =
                super::source::files(record["snapshot"]["patch"].as_str().unwrap_or_default());
            record
        });
        return json_response(stream, result);
    }
    if request.method == "POST" && path == "api" {
        if !request.content_type.starts_with("application/json") {
            return respond(stream, 415, "text/plain", b"Expected application/json");
        }
        let input: Value = serde_json::from_slice(&request.body)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let method = input["method"].as_str().unwrap_or("");
        if !matches!(
            method,
            "comment.add"
                | "comment.reply"
                | "comment.edit"
                | "comment.remove"
                | "feedback.submit"
                | "review.finish"
                | "review.refresh"
                | "review.disposition"
                | "review.progress"
        ) {
            return respond(stream, 403, "text/plain", b"Unsupported viewer operation");
        }
        let mut params = input.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected parameter object",
            ));
        }
        // The URL grants access to one review, never arbitrary local reviews.
        params["review_id"] = json!(review_id);
        if matches!(method, "comment.add" | "comment.reply") {
            params["origin"] = json!("human");
            params["author"] = Value::Null;
        }
        return json_response(stream, super::call(method, params));
    }
    respond(stream, 404, "text/plain", b"Review not found")
}

fn json_response(stream: &mut TcpStream, result: io::Result<Value>) -> io::Result<()> {
    let (status, body) = match result {
        Ok(result) => (200, json!({"ok":true,"result":result})),
        Err(error) => (
            409,
            json!({"ok":false,"error":{"message":error.to_string()}}),
        ),
    };
    respond(
        stream,
        status,
        "application/json",
        body.to_string().as_bytes(),
    )
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} Response\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

struct Request {
    method: String,
    path: String,
    host: String,
    origin: Option<String>,
    content_type: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> io::Result<Request> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid or oversized HTTP request",
        )
    };
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(offset) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            break offset + 4;
        }
        if bytes.len() >= MAX_HEADER {
            return Err(invalid());
        }
        let mut chunk = [0u8; 1024];
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(invalid());
        }
        bytes.extend_from_slice(&chunk[..count]);
    };
    if header_end > MAX_HEADER {
        return Err(invalid());
    }
    let header = std::str::from_utf8(&bytes[..header_end]).map_err(|_| invalid())?;
    let mut lines = header.split("\r\n");
    let mut start = lines.next().ok_or_else(invalid)?.split_whitespace();
    let method = start.next().ok_or_else(invalid)?.to_owned();
    let path = start.next().ok_or_else(invalid)?.to_owned();
    if start.next() != Some("HTTP/1.1") || start.next().is_some() {
        return Err(invalid());
    }
    let mut host = None;
    let mut origin = None;
    let mut length = None;
    let mut content_type = String::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once(':').ok_or_else(invalid)?;
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                if host.replace(value.to_owned()).is_some() {
                    return Err(invalid());
                }
            }
            "origin" => {
                if origin.replace(value.to_owned()).is_some() {
                    return Err(invalid());
                }
            }
            "content-length" => {
                if length
                    .replace(value.parse::<usize>().map_err(|_| invalid())?)
                    .is_some()
                {
                    return Err(invalid());
                }
            }
            "content-type" => content_type = value.to_owned(),
            "transfer-encoding" => return Err(invalid()),
            _ => {}
        }
    }
    let length = length.unwrap_or(0);
    if length > MAX_BODY {
        return Err(invalid());
    }
    while bytes.len() - header_end < length {
        let mut chunk = [0u8; 4096];
        let remaining = (length - (bytes.len() - header_end)).min(chunk.len());
        let count = stream.read(&mut chunk[..remaining])?;
        if count == 0 {
            return Err(invalid());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(Request {
        method,
        path,
        host: host.ok_or_else(invalid)?,
        origin,
        content_type,
        body: bytes[header_end..header_end + length].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        raw: &[u8],
        handler: impl FnOnce(&mut TcpStream) -> io::Result<()> + Send + 'static,
    ) -> Vec<u8> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handler(&mut stream)
        });
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(raw).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        worker.join().unwrap().unwrap();
        response
    }

    #[test]
    fn viewer_rejects_foreign_host_origin_and_missing_capability() {
        for raw in [
            "GET /token/ HTTP/1.1\r\nHost: attacker.example\r\n\r\n",
            "GET /token/ HTTP/1.1\r\nHost: 127.0.0.1:42\r\nOrigin: https://attacker.example\r\n\r\n",
        ] {
            let response = request(raw.as_bytes(), |stream| {
                connection(
                    stream,
                    "review",
                    "/token/",
                    "127.0.0.1:42",
                    "http://127.0.0.1:42",
                )
            });
            assert!(response.starts_with(b"HTTP/1.1 403"));
        }
        let response = request(b"GET / HTTP/1.1\r\nHost: 127.0.0.1:42\r\n\r\n", |stream| {
            connection(
                stream,
                "review",
                "/token/",
                "127.0.0.1:42",
                "http://127.0.0.1:42",
            )
        });
        assert!(response.starts_with(b"HTTP/1.1 404"));
    }

    #[test]
    fn viewer_serves_without_external_assets() {
        let response = request(
            b"GET /token/ HTTP/1.1\r\nHost: 127.0.0.1:42\r\n\r\n",
            |stream| {
                connection(
                    stream,
                    "review",
                    "/token/",
                    "127.0.0.1:42",
                    "http://127.0.0.1:42",
                )
            },
        );
        assert!(response.starts_with(b"HTTP/1.1 200"));
        assert!(String::from_utf8_lossy(&response).contains("Send comments"));
    }

    #[test]
    fn duplicate_lengths_and_oversized_bodies_are_rejected() {
        for raw in [
            "POST / HTTP/1.1\r\nHost: local\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n",
            "POST / HTTP/1.1\r\nHost: local\r\nContent-Length: 2000000\r\n\r\n",
        ] {
            request(raw.as_bytes(), |stream| {
                assert!(read_request(stream).is_err());
                Ok(())
            });
        }
    }
}

#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    time::Duration,
};

use mark_session::{
    Client, METHOD_SESSION_GET, PROTOCOL_VERSION, Registry, Request, Response,
    SESSION_COMMAND_CHANNEL_CAPACITY, SessionRecord, current_process_identity, spawn_server,
};
use tokio::sync::mpsc;

#[test]
fn one_request_round_trip_and_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Registry::at(temp.path().join("mark"));
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let session_id = "round-trip";
    let record = SessionRecord {
        session_id: session_id.to_owned(),
        process_id: std::process::id(),
        process_identity: current_process_identity(),
        protocol: PROTOCOL_VERSION,
        repository: fs::canonicalize(&repo).unwrap().display().to_string(),
        working_directory: repo.display().to_string(),
        source: "worktree".to_owned(),
        endpoint: registry
            .socket_path(session_id)
            .unwrap()
            .display()
            .to_string(),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (tx, mut rx) = mpsc::channel(SESSION_COMMAND_CHANNEL_CAPACITY);
    let (handle, startup) = runtime.block_on(async { spawn_server(registry.clone(), record, tx) });
    runtime
        .block_on(startup)
        .expect("startup signal")
        .expect("server startup");
    assert_eq!(
        fs::metadata(registry.session_dir(session_id).unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(registry.socket_path(session_id).unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    runtime.spawn(async move {
        if let Some(command) = rx.recv().await {
            let _ = command.reply.send(Response::success(
                command.request.id,
                serde_json::json!({"ready": true}),
            ));
        }
    });

    let socket_path = registry.socket_path(session_id).unwrap();
    let mut partial = std::os::unix::net::UnixStream::connect(&socket_path).unwrap();
    partial.write_all(b"{").unwrap();
    partial.shutdown(std::net::Shutdown::Write).unwrap();
    let mut partial_response = String::new();
    partial.read_to_string(&mut partial_response).unwrap();
    let partial_response: Response = serde_json::from_str(partial_response.trim()).unwrap();
    assert_eq!(partial_response.error.unwrap().code, "invalid_frame");

    let response = Client::new(socket_path)
        .with_timeout(Duration::from_secs(2))
        .request(&Request::new(
            "request-1",
            METHOD_SESSION_GET,
            serde_json::json!({}),
        ))
        .unwrap();
    assert!(response.ok);
    assert_eq!(response.result.unwrap()["ready"], true);

    drop(handle);
    runtime.block_on(tokio::task::yield_now());
    for _ in 0..100 {
        if !registry.session_dir(session_id).unwrap().exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!registry.session_dir(session_id).unwrap().exists());
}

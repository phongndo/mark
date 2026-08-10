#![cfg(unix)]

use std::{fs, os::unix::net::UnixListener};

use mark_session::{PROTOCOL_VERSION, Registry, SessionRecord, current_process_identity};
use tempfile::tempdir;

fn record(registry: &Registry, id: &str, repo: &str) -> SessionRecord {
    SessionRecord {
        session_id: id.to_owned(),
        process_id: std::process::id(),
        process_identity: current_process_identity(),
        protocol: PROTOCOL_VERSION,
        repository: repo.to_owned(),
        working_directory: repo.to_owned(),
        source: "worktree".to_owned(),
        endpoint: registry.socket_path(id).unwrap().display().to_string(),
    }
}

#[test]
fn private_record_is_discovered_and_selected_deterministically() {
    let temp = tempdir().unwrap();
    let registry = Registry::at(temp.path().join("mark"));
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let repo = fs::canonicalize(repo).unwrap();
    registry.prepare_session("session-a").unwrap();
    let listener = UnixListener::bind(registry.socket_path("session-a").unwrap()).unwrap();
    registry
        .write_record(&record(&registry, "session-a", repo.to_str().unwrap()))
        .unwrap();

    let sessions = registry.list().unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].responsive);
    assert_eq!(
        registry
            .select(None, Some(&repo))
            .unwrap()
            .record
            .session_id,
        "session-a"
    );

    drop(listener);
    registry.remove_session("session-a").unwrap();
    assert!(registry.list().unwrap().is_empty());
}

#[test]
fn listing_preserves_a_registration_until_its_record_is_published() {
    let temp = tempdir().unwrap();
    let registry = Registry::at(temp.path().join("mark"));
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let repo = fs::canonicalize(repo).unwrap();
    let session_id = "starting";
    registry.prepare_session(session_id).unwrap();
    let socket_path = registry.socket_path(session_id).unwrap();
    let listener = UnixListener::bind(&socket_path).unwrap();

    assert!(registry.list().unwrap().is_empty());
    assert!(registry.session_dir(session_id).unwrap().is_dir());
    assert!(socket_path.exists());

    registry
        .write_record(&record(&registry, session_id, repo.to_str().unwrap()))
        .unwrap();
    let sessions = registry.list().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].record.session_id, session_id);
    assert!(sessions[0].responsive);

    drop(listener);
    registry.remove_session(session_id).unwrap();
}

#[test]
fn stale_records_are_removed() {
    let temp = tempdir().unwrap();
    let registry = Registry::at(temp.path().join("mark"));
    registry.prepare_session("stale").unwrap();
    registry
        .write_record(&record(&registry, "stale", temp.path().to_str().unwrap()))
        .unwrap();

    assert!(registry.list().unwrap().is_empty());
    assert!(!registry.session_dir("stale").unwrap().exists());
}

#[test]
fn registry_rejects_symlinked_base_directory() {
    let temp = tempdir().unwrap();
    let target = temp.path().join("target");
    fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, temp.path().join("mark")).unwrap();
    let registry = Registry::at(temp.path().join("mark"));

    assert!(registry.prepare_session("unsafe").is_err());
}

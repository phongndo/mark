use std::{fs, process::Command};

#[test]
fn skill_path_and_show_materialize_identical_versioned_content() {
    let temp = tempfile::tempdir().expect("temp cache");
    let binary = env!("CARGO_BIN_EXE_mark");
    let show = Command::new(binary)
        .args(["skill", "show"])
        .output()
        .expect("skill show should run");
    assert!(show.status.success());

    let path = Command::new(binary)
        .args(["skill", "path"])
        .env("XDG_CACHE_HOME", temp.path())
        .output()
        .expect("skill path should run");
    assert!(path.status.success());
    let path = String::from_utf8(path.stdout).expect("path should be UTF-8");
    let path = std::path::Path::new(path.trim());
    assert!(path.to_string_lossy().contains(env!("CARGO_PKG_VERSION")));
    assert_eq!(fs::read(path).expect("materialized skill"), show.stdout);
}

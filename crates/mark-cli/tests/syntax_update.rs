use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn syntax_update_all_warns_for_unavailable_configured_languages() {
    let dir = unique_temp_dir("syntax-update-all");
    let mark_dir = dir.join("mark");
    fs::create_dir_all(&mark_dir).unwrap();
    fs::write(
        mark_dir.join("syntax.json"),
        r#"{"languages": ["definitely_missing"]}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mark"))
        .args(["syntax", "update", "--all"])
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .expect("mark syntax update --all should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert!(
        stdout.contains("warning definitely_missing: no bundled grammar"),
        "stdout: {stdout}"
    );

    remove_temp_dir(&dir);
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("mark-cli-{label}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn remove_temp_dir(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

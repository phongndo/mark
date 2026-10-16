use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn syntax_inspect_rejects_lines_outside_the_file() {
    let dir = unique_temp_dir("syntax-inspect");
    let source = dir.join("source.rs");
    fs::write(&source, "fn main() {}\n").unwrap();

    let first_line = inspect(&dir, &source, "1");
    assert!(
        first_line.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first_line.stderr)
    );
    assert!(!first_line.stdout.is_empty());

    for line in ["0", "99"] {
        let output = inspect(&dir, &source, line);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "--line {line} should fail");
        assert!(output.stdout.is_empty());
        assert!(
            stderr.contains(&format!("line {line} is outside")),
            "stderr: {stderr}"
        );
    }

    remove_temp_dir(&dir);
}

fn inspect(dir: &Path, source: &Path, line: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mark"))
        .args(["syntax", "inspect"])
        .arg(source)
        .args(["--line", line])
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .output()
        .expect("mark syntax inspect should run")
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

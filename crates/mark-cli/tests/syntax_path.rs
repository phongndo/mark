use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn syntax_path_prints_current_config_paths() {
    let dir = unique_temp_dir("syntax-path");

    let output = Command::new(env!("CARGO_BIN_EXE_mark"))
        .args(["syntax", "path"])
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .expect("mark syntax path should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mark_dir = dir.join("mark");
    let expected = format!(
        "mappings    {}\nconfig      {}\ncolorscheme {}\n",
        mark_dir.join("syntax.json").display(),
        mark_dir.join("config.toml").display(),
        mark_dir.join("colorscheme").display()
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert_eq!(stdout, expected);
    assert!(!stdout.contains("cache"));
    assert!(!stdout.contains("registry"));

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

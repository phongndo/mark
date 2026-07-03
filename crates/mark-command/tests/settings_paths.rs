use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn settings_path_commands_preserve_active_legacy_settings_file() {
    let dir = unique_temp_dir("legacy-settings-path");

    with_xdg_config_home(&dir, || {
        let mark_dir = dir.join("mark");
        fs::create_dir_all(&mark_dir).unwrap();
        let legacy_path = mark_dir.join("syntax.toml");
        fs::write(&legacy_path, "colorscheme = \"ansi\"\n").unwrap();

        assert_eq!(mark_command::config_path().unwrap(), legacy_path);
        assert_eq!(mark_command::syntax_settings_path().unwrap(), legacy_path);
    });

    remove_temp_dir(&dir);
}

#[test]
fn settings_path_commands_prefer_config_toml_when_present() {
    let dir = unique_temp_dir("config-settings-path");

    with_xdg_config_home(&dir, || {
        let mark_dir = dir.join("mark");
        fs::create_dir_all(&mark_dir).unwrap();
        let config_path = mark_dir.join("config.toml");
        let legacy_path = mark_dir.join("syntax.toml");
        fs::write(&config_path, "colorscheme = \"system\"\n").unwrap();
        fs::write(&legacy_path, "colorscheme = \"ansi\"\n").unwrap();

        assert_eq!(mark_command::config_path().unwrap(), config_path);
        assert_eq!(mark_command::syntax_settings_path().unwrap(), config_path);
    });

    remove_temp_dir(&dir);
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "mark-command-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn remove_temp_dir(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn with_xdg_config_home<T>(path: &Path, f: impl FnOnce() -> T) -> T {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let previous = std::env::var_os("XDG_CONFIG_HOME");

    // SAFETY: tests that mutate XDG_CONFIG_HOME serialize through ENV_LOCK and
    // do not spawn threads while the temporary value is installed.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", path) };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

    // SAFETY: guarded by ENV_LOCK as above; restore the previous process
    // environment value before allowing another test to continue.
    unsafe {
        match previous {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }

    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

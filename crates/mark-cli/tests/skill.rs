use std::{fs, process::Command};

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mark"))
        .args(args)
        .output()
        .expect("mark skill command should run")
}

#[test]
fn bare_skill_and_show_print_identical_content() {
    let bare = run(&["skill"]);
    let show = run(&["skill", "show"]);
    assert!(bare.status.success());
    assert!(show.status.success());
    assert_eq!(bare.stdout, show.stdout);
}

#[test]
fn skill_path_and_show_materialize_identical_versioned_content() {
    let temp = tempfile::tempdir().expect("temp cache");
    let show = run(&["skill", "show"]);
    assert!(show.status.success());

    let path = Command::new(env!("CARGO_BIN_EXE_mark"))
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

#[test]
fn skill_install_uses_each_agents_user_skill_directory() {
    let temp = tempfile::tempdir().expect("temp home");
    let show = run(&["skill", "show"]);
    assert!(show.status.success());

    for (agent, relative_path) in [
        ("pi", ".pi/agent/skills/mark-live-review/SKILL.md"),
        ("codex", ".agents/skills/mark-live-review/SKILL.md"),
        ("claude", ".claude/skills/mark-live-review/SKILL.md"),
        ("cursor", ".cursor/skills/mark-live-review/SKILL.md"),
        (
            "antigravity",
            ".gemini/config/skills/mark-live-review/SKILL.md",
        ),
        ("copilot", ".copilot/skills/mark-live-review/SKILL.md"),
        (
            "opencode",
            ".config/opencode/skills/mark-live-review/SKILL.md",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mark"))
            .args(["skill", "install", "--agent", agent])
            .env("HOME", temp.path())
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("skill install should run");
        assert!(output.status.success(), "install failed for {agent}");
        let expected = temp.path().join(relative_path);
        assert_eq!(
            String::from_utf8(output.stdout).expect("path should be UTF-8"),
            format!("{}\n", expected.display())
        );
        assert_eq!(fs::read(expected).expect("installed skill"), show.stdout);
    }
}

#[test]
fn opencode_install_respects_xdg_config_home() {
    let home = tempfile::tempdir().expect("temp home");
    let config = tempfile::tempdir().expect("temp config home");
    let output = Command::new(env!("CARGO_BIN_EXE_mark"))
        .args(["skill", "install", "--agent", "opencode"])
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", config.path())
        .output()
        .expect("skill install should run");
    assert!(output.status.success());
    let expected = config
        .path()
        .join("opencode/skills/mark-live-review/SKILL.md");
    assert_eq!(
        String::from_utf8(output.stdout).expect("path should be UTF-8"),
        format!("{}\n", expected.display())
    );
    assert!(expected.is_file());
}

#[test]
fn skill_install_requires_one_supported_agent() {
    let missing = run(&["skill", "install"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--agent"));

    let all = run(&["skill", "install", "--agent", "all"]);
    assert!(!all.status.success());
}

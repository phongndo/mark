use super::*;
use std::fs;

const AGENTS: [IntegrationAgent; 5] = [
    IntegrationAgent::Pi,
    IntegrationAgent::Claude,
    IntegrationAgent::Codex,
    IntegrationAgent::Opencode,
    IntegrationAgent::Fx,
];

fn tempdir() -> tempfile::TempDir {
    // macOS commonly spells TMPDIR through the /var -> /private/var alias.
    // The export itself deliberately refuses any symlink in its input path.
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

#[test]
fn packages_are_self_contained_outside_the_checkout_and_match_the_binary_version() {
    let temporary = tempdir();
    assert!(
        EMBEDDED
            .iter()
            .all(|(name, _)| runtime_asset(Path::new(name)))
    );
    for agent in AGENTS {
        let output = materialize(agent, &temporary.path().join(agent.name())).unwrap();
        let manifest: Value =
            serde_json::from_slice(&fs::read(output.join(agent.manifest())).unwrap()).unwrap();
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
        let skill = output.join("skills/mark/SKILL.md");
        assert_eq!(
            skill.is_file(),
            matches!(agent, IntegrationAgent::Codex | IntegrationAgent::Fx)
        );
        assert!(!output.join(".mcp.json").exists());
        assert!(output.join("common/review.mjs").is_file());
        assert_eq!(
            fs::read(output.join("LICENSE")).unwrap(),
            LICENSE.as_bytes()
        );
        let intro = fs::read_to_string(output.join("README.md")).unwrap();
        assert!(intro.split_whitespace().count() >= 40);
        assert!(intro.contains("docs/integrations.md"));
        for (name, bytes) in package(agent).unwrap() {
            assert!(
                !fs::symlink_metadata(output.join(&name))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            if !matches!(
                name.extension().and_then(|value| value.to_str()),
                Some("js" | "mjs")
            ) {
                continue;
            }
            let source = text(&bytes).unwrap();
            for line in source.lines().filter(|line| line.starts_with("import ")) {
                let Some(import) = line.split('"').nth(1) else {
                    continue;
                };
                if !import.starts_with('.') {
                    continue;
                }
                let target = output
                    .join(name.parent().unwrap())
                    .join(import)
                    .canonicalize()
                    .unwrap_or_else(|error| panic!("{} import {import}: {error}", name.display()));
                assert!(
                    target.starts_with(&output),
                    "{} escapes the package",
                    name.display()
                );
            }
        }
        if matches!(agent, IntegrationAgent::Codex | IntegrationAgent::Fx) {
            assert!(output.join("skills/mark/HOST.md").is_file());
        }
    }
}

#[test]
fn fx_skill_survives_without_its_surrounding_package() {
    let temporary = tempdir();
    let output = materialize(IntegrationAgent::Fx, &temporary.path().join("package")).unwrap();
    let detached = temporary.path().join("detached");
    fs::create_dir(&detached).unwrap();
    for (path, bytes) in package(IntegrationAgent::Fx).unwrap() {
        if let Ok(relative) = path.strip_prefix("skills/mark") {
            let target = detached.join(relative);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, bytes).unwrap();
        }
    }
    fs::remove_dir_all(output).unwrap();
    assert!(detached.join("open-review.mjs").is_file());
    assert!(detached.join("common/review.mjs").is_file());
    assert!(detached.join("LICENSE").is_file());
    assert!(detached.join("HOST.md").is_file());
    assert!(
        fs::read_to_string(detached.join("open-review.mjs"))
            .unwrap()
            .contains("\"./common/review.mjs\"")
    );
}

#[test]
fn repeated_materialization_reuses_identical_files() {
    let temporary = tempdir();
    let output = temporary.path().join("pi");
    assert_eq!(materialize(IntegrationAgent::Pi, &output).unwrap(), output);
    let metadata = fs::metadata(output.join("index.js")).unwrap();
    let content = fs::read(output.join("index.js")).unwrap();
    materialize(IntegrationAgent::Pi, &output).unwrap();
    assert_eq!(
        metadata.modified().unwrap(),
        fs::metadata(output.join("index.js"))
            .unwrap()
            .modified()
            .unwrap()
    );
    assert_eq!(fs::read(output.join("index.js")).unwrap(), content);
}

#[test]
fn edited_missing_or_extra_files_are_preserved_and_reported_before_writes() {
    let temporary = tempdir();
    let edited = materialize(IntegrationAgent::Pi, &temporary.path().join("edited")).unwrap();
    fs::write(edited.join("index.js"), "user changes").unwrap();
    let error = materialize(IntegrationAgent::Pi, &edited).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert!(error.to_string().starts_with("integration_modified:"));
    assert_eq!(
        fs::read_to_string(edited.join("index.js")).unwrap(),
        "user changes"
    );
    let removed = materialize(IntegrationAgent::Fx, &temporary.path().join("removed")).unwrap();
    fs::remove_file(removed.join("open-review.mjs")).unwrap();
    assert!(materialize(IntegrationAgent::Fx, &removed).is_err());
    assert!(!removed.join("open-review.mjs").exists());
    let extra = temporary.path().join("extra");
    fs::create_dir(&extra).unwrap();
    fs::write(extra.join("notes.txt"), "keep me").unwrap();
    assert!(materialize(IntegrationAgent::Fx, &extra).is_err());
    assert!(!extra.join("open-review.mjs").exists());
    assert_eq!(
        fs::read_to_string(extra.join("notes.txt")).unwrap(),
        "keep me"
    );
}

#[test]
fn native_marketplaces_and_portable_codex_metadata_reference_this_package() {
    let temporary = tempdir();
    let claude = materialize(IntegrationAgent::Claude, &temporary.path().join("claude")).unwrap();
    let catalog: Value =
        serde_json::from_slice(&fs::read(claude.join(".claude-plugin/marketplace.json")).unwrap())
            .unwrap();
    assert_eq!(catalog["plugins"][0]["name"], "mark");
    assert!(
        claude
            .join(catalog["plugins"][0]["source"].as_str().unwrap())
            .is_dir()
    );
    let codex = materialize(IntegrationAgent::Codex, &temporary.path().join("codex")).unwrap();
    let catalog: Value =
        serde_json::from_slice(&fs::read(codex.join(".agents/plugins/marketplace.json")).unwrap())
            .unwrap();
    assert_eq!(catalog["plugins"][0]["source"]["source"], "local");
    assert_eq!(catalog["plugins"][0]["policy"]["installation"], "AVAILABLE");
    assert!(
        codex
            .join(catalog["plugins"][0]["source"]["path"].as_str().unwrap())
            .is_dir()
    );
    let portable: Value =
        serde_json::from_slice(&fs::read(codex.join("plugin.json")).unwrap()).unwrap();
    assert_eq!(portable["version"], env!("CARGO_PKG_VERSION"));
    assert!(!codex.join("mcp.json").exists());
    assert!(!codex.join(".mcp.json").exists());
}

#[test]
fn integration_commands_parse_all_supported_agents() {
    use clap::Parser;
    for agent in AGENTS {
        crate::args::Cli::try_parse_from(["mark", "integration", "path", "--agent", agent.name()])
            .unwrap();
        crate::args::Cli::try_parse_from([
            "mark",
            "integration",
            "export",
            "--agent",
            agent.name(),
            "--output",
            "package",
        ])
        .unwrap();
    }
    assert!(crate::args::Cli::try_parse_from(["mark", "integration", "path"]).is_err());
    assert!(
        crate::args::Cli::try_parse_from(["mark", "integration", "path", "--agent", "unknown"])
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn exported_packages_are_private_and_symlink_traversal_is_refused() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temporary = tempdir();
    let output = materialize(IntegrationAgent::Fx, &temporary.path().join("fx")).unwrap();
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o077,
        0
    );
    assert_eq!(
        fs::metadata(output.join("open-review.mjs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o077,
        0
    );
    let outside = temporary.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let linked = temporary.path().join("linked");
    symlink(&outside, &linked).unwrap();
    assert!(materialize(IntegrationAgent::Fx, &linked.join("package")).is_err());
    assert!(!outside.join("package").exists());
    fs::remove_file(output.join("open-review.mjs")).unwrap();
    let target = outside.join("untouched");
    fs::write(&target, "untouched").unwrap();
    symlink(&target, output.join("open-review.mjs")).unwrap();
    assert!(materialize(IntegrationAgent::Fx, &output).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
}

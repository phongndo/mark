use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    embed_integrations();
    println!("cargo:rerun-if-env-changed=MARK_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=MARK_BUILD_COMMIT");

    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION should be set");
    let Some(channel) = env::var("MARK_BUILD_CHANNEL")
        .ok()
        .map(|channel| sanitize_version_component(&channel))
        .filter(|channel| !channel.is_empty())
    else {
        println!("cargo:rustc-env=MARK_CLI_VERSION={version}");
        return;
    };

    let commit = env::var("MARK_BUILD_COMMIT")
        .ok()
        .map(|commit| sanitize_version_component(&short_commit(&commit)))
        .filter(|commit| !commit.is_empty());

    let version = match commit {
        Some(commit) => format!("{version}-{channel}+{commit}"),
        None => format!("{version}-{channel}"),
    };
    println!("cargo:rustc-env=MARK_CLI_VERSION={version}");
}

fn embed_integrations() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("crate directory"));
    let root = manifest.join("../../assets/integrations");
    let mut files = Vec::new();
    for directory in ["common", "pi", "claude", "codex", "opencode", "fx"] {
        collect_assets(&root.join(directory), &mut files);
    }
    files.sort();
    let mut index = String::from("const EMBEDDED: &[(&str, &[u8])] = &[\n");
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .expect("asset inside integration directory");
        let name = relative
            .to_str()
            .expect("UTF-8 asset path")
            .replace('\\', "/");
        println!("cargo:rerun-if-changed={}", path.display());
        index.push_str(&format!("({name:?}, include_bytes!({path:?})),\n"));
    }
    index.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"))
            .join("integration_assets.rs"),
        index,
    )
    .expect("write embedded integration index");
}

fn collect_assets(directory: &Path, files: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", directory.display());
    for entry in fs::read_dir(directory).expect("read integration assets") {
        let entry = entry.expect("integration asset entry");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            "tests"
                | "__tests__"
                | "node_modules"
                | ".git"
                | "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | ".DS_Store"
        ) || name.contains(".test.")
            || name.contains(".spec.")
        {
            continue;
        }
        let kind = entry.file_type().expect("integration asset file type");
        assert!(
            !kind.is_symlink(),
            "integration assets must not be symlinks: {}",
            entry.path().display()
        );
        if kind.is_dir() {
            collect_assets(&entry.path(), files);
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
}

fn sanitize_version_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '0'..='9' | 'A'..='Z' | 'a'..='z' | '-' | '.' => ch,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches(['-', '.'])
        .to_owned()
}

fn short_commit(value: &str) -> String {
    value.chars().take(12).collect()
}

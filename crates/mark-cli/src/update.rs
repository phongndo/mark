use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitStatus, Stdio},
};

use mark_core::{MarkError, MarkResult};

use crate::{
    CliResult,
    args::{INSTALL_SCRIPT, RELEASE_REPO, UpdateArgs},
    version::CLI_VERSION,
    write_stderr,
};

/// The mise tool and Nix flake that the README installs.
const MISE_TOOL: &str = "github:phongndo/mark";
const NIX_FLAKE: &str = "github:phongndo/mark";

pub(crate) fn update(args: UpdateArgs) -> CliResult<()> {
    let argv0 = env::args_os()
        .next()
        .ok_or_else(|| usage_error("could not determine current executable"))?;
    let binary = update_binary_name(&argv0)?;
    let install_dir = match args.install_dir {
        Some(path) => absolute_path(path)?,
        None => default_update_install_dir(&argv0)?,
    };
    let mise_data_dir = mise_data_dir(env::var_os("MISE_DATA_DIR"));
    let target_manager = managed_update_install(&install_dir, &binary, mise_data_dir.as_deref());
    let executable = env::current_exe().and_then(fs::canonicalize).ok();

    match classify_update_method(
        executable.as_deref(),
        mise_data_dir.as_deref(),
        target_manager,
    ) {
        UpdateMethod::Installer => run_installer(&install_dir, &binary, args.version),
        UpdateMethod::Homebrew { prefix, formula } => {
            if pinned_version(args.version.as_deref()).is_some() {
                return Err(usage_error(
                    "Homebrew installs only the latest release; run `mark update` without --target-version.",
                )
                .into());
            }
            let brew = prefix.join("bin").join("brew");
            let program = if is_executable(&brew) {
                brew.into_os_string()
            } else {
                OsString::from("brew")
            };
            run_package_manager(&program, &["upgrade", &formula])
        }
        UpdateMethod::Mise => {
            if let Some(version) = pinned_version(args.version.as_deref()) {
                return Err(usage_error(format!(
                    "pin a version with mise instead: mise use -g {MISE_TOOL}@{version}"
                ))
                .into());
            }
            run_package_manager(OsStr::new("mise"), &["upgrade", MISE_TOOL])
        }
        UpdateMethod::Nix { store_dir } => {
            let Some(name) =
                nix_profile_list_json().and_then(|json| nix_profile_element(&json, &store_dir))
            else {
                return Err(usage_error(
                    "this mark comes from a Nix configuration or `nix run`; update its flake input (nix flake update mark) and rebuild.",
                )
                .into());
            };
            if let Some(version) = pinned_version(args.version.as_deref()) {
                return Err(usage_error(format!(
                    "pin a version with Nix instead: nix profile remove {name} && nix profile add {NIX_FLAKE}/v{version}"
                ))
                .into());
            }
            run_package_manager(OsStr::new("nix"), &["profile", "upgrade", &name])
        }
        UpdateMethod::Refuse(manager) => {
            Err(managed_update_refusal(&install_dir, &binary, manager).into())
        }
    }
}

fn run_installer(install_dir: &Path, binary: &OsStr, version: Option<String>) -> CliResult<()> {
    let updates_current_executable = update_target_is_current_executable(install_dir, binary);
    let version = version.unwrap_or_else(|| "latest".to_owned());
    let repo = update_repo(env::var_os("MARK_REPO"));

    let mut command = ProcessCommand::new("sh");
    command
        .arg("-s")
        .env("MARK_REPO", repo)
        .env("MARK_INSTALL_DIR", install_dir)
        .env("MARK_VERSION", version)
        .env_remove("MARK_CURRENT_VERSION")
        .env("MARK_BINARY", binary)
        .env("MARK_INSTALL_ACTION", "update")
        .stdin(Stdio::piped());
    if updates_current_executable {
        command.env("MARK_CURRENT_VERSION", CLI_VERSION);
    }
    let mut child = command.spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| usage_error("could not open installer stdin"))?;
    stdin.write_all(INSTALL_SCRIPT.as_bytes())?;
    drop(stdin);

    update_status(child.wait()?)
}

/// Print the package manager command, then replace this process with it.
fn run_package_manager(program: &OsStr, args: &[&str]) -> CliResult<()> {
    let program_name = program.to_string_lossy();
    let command_line = format!("{program_name} {}", args.join(" "));
    write_stderr(format_args!("Updating with: {command_line}\n"))?;

    let mut command = ProcessCommand::new(program);
    command.args(args);
    let status = replace_process(&mut command).map_err(|error| {
        usage_error(format!(
            "could not run {program_name}: {error}. Run it yourself: {command_line}"
        ))
    })?;
    update_status(status)
}

/// Only returns if the command could not start.
#[cfg(unix)]
fn replace_process(command: &mut ProcessCommand) -> io::Result<ExitStatus> {
    use std::os::unix::process::CommandExt;

    Err(command.exec())
}

#[cfg(not(unix))]
fn replace_process(command: &mut ProcessCommand) -> io::Result<ExitStatus> {
    command.status()
}

fn update_status(status: ExitStatus) -> CliResult<()> {
    if status.success() {
        return Ok(());
    }

    Err(usage_error(format!(
        "update failed with status {}",
        status
            .code()
            .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
    ))
    .into())
}

fn usage_error(message: impl Into<String>) -> MarkError {
    MarkError::Usage(message.into())
}

/// A version to pin, without the leading v. `latest` is what package managers
/// install anyway, so it is not a pin.
pub(crate) fn pinned_version(version: Option<&str>) -> Option<&str> {
    version
        .filter(|version| !version.is_empty() && *version != "latest")
        .map(|version| version.strip_prefix('v').unwrap_or(version))
}

pub(crate) fn mise_data_dir(value: Option<OsString>) -> Option<PathBuf> {
    let dir = PathBuf::from(value.filter(|value| !value.is_empty())?);
    Some(fs::canonicalize(&dir).unwrap_or(dir))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn update_repo(repo: Option<OsString>) -> OsString {
    repo.filter(|repo| !repo.as_os_str().is_empty())
        .unwrap_or_else(|| OsString::from(RELEASE_REPO))
}

pub(crate) fn update_binary_name(argv0: &OsStr) -> MarkResult<OsString> {
    Path::new(argv0)
        .file_name()
        .filter(|name| !name.is_empty())
        .map(OsString::from)
        .ok_or_else(|| usage_error("could not determine current executable name"))
}

/// How `mark update` updates the target mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateMethod {
    /// Rerun the install script on the install directory.
    Installer,
    /// `<prefix>/bin/brew upgrade <formula>`.
    Homebrew { prefix: PathBuf, formula: String },
    /// `mise upgrade github:phongndo/mark`.
    Mise,
    /// `nix profile upgrade` the profile element that contains this store path.
    Nix { store_dir: PathBuf },
    /// Print how to update with the manager that owns the target.
    Refuse(ManagedUpdateInstall),
}

/// Chooses how to update. `target_manager` is what the install directory
/// checks found; without one, the target is an install-script install. With
/// one, the running executable's canonical path decides which package manager
/// to run, and only when it agrees with those checks, so that `--install-dir`
/// or a copied binary never upgrades a different install.
pub(crate) fn classify_update_method(
    executable: Option<&Path>,
    mise_data_dir: Option<&Path>,
    target_manager: Option<ManagedUpdateInstall>,
) -> UpdateMethod {
    let Some(manager) = target_manager else {
        return UpdateMethod::Installer;
    };

    let method = executable.and_then(|executable| {
        if let Some((prefix, formula)) = homebrew_cellar_install(executable) {
            Some((
                ManagedUpdateInstall::Homebrew,
                UpdateMethod::Homebrew { prefix, formula },
            ))
        } else if is_mise_install(executable, mise_data_dir) {
            Some((ManagedUpdateInstall::Mise, UpdateMethod::Mise))
        } else {
            nix_store_dir(executable)
                .map(|store_dir| (ManagedUpdateInstall::Nix, UpdateMethod::Nix { store_dir }))
        }
    });

    match method {
        Some((executable_manager, method)) if executable_manager == manager => method,
        _ => UpdateMethod::Refuse(manager),
    }
}

/// `<prefix>/Cellar/<formula>/<version>/...` gives the prefix and formula.
fn homebrew_cellar_install(path: &Path) -> Option<(PathBuf, String)> {
    let components = path.components().collect::<Vec<_>>();
    let cellar = components
        .iter()
        .position(|component| component.as_os_str() == "Cellar")?;
    if cellar == 0 || components.len() < cellar + 4 {
        return None;
    }

    let formula = components[cellar + 1].as_os_str().to_str()?.to_owned();
    Some((components[..cellar].iter().collect(), formula))
}

fn is_mise_install(path: &Path, mise_data_dir: Option<&Path>) -> bool {
    match mise_data_dir {
        Some(dir) => path.starts_with(dir.join("installs")),
        None => path.to_string_lossy().contains("/mise/installs/"),
    }
}

/// `/nix/store/<hash>-<name>` for a path inside the Nix store.
fn nix_store_dir(path: &Path) -> Option<PathBuf> {
    let store = Path::new("/nix/store");
    let entry = path.strip_prefix(store).ok()?.components().next()?;
    Some(store.join(entry))
}

fn nix_profile_list_json() -> Option<String> {
    let output = ProcessCommand::new("nix")
        .args(["profile", "list", "--json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// The name of the `nix profile list --json` element that contains
/// `store_dir`. Guessing instead is unsafe: `nix profile upgrade` with an
/// unknown name only warns and exits successfully.
pub(crate) fn nix_profile_element(json: &str, store_dir: &Path) -> Option<String> {
    let profile = serde_json::from_str::<serde_json::Value>(json).ok()?;
    profile
        .get("elements")?
        .as_object()?
        .iter()
        .find(|(_, element)| {
            element
                .get("storePaths")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|paths| {
                    paths
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .any(|path| Path::new(path) == store_dir)
                })
        })
        .map(|(name, _)| name.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedUpdateInstall {
    Homebrew,
    Mise,
    Cargo,
    Nix,
    Asdf,
}

impl ManagedUpdateInstall {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Homebrew => "Homebrew",
            Self::Mise => "mise",
            Self::Cargo => "Cargo",
            Self::Nix => "Nix",
            Self::Asdf => "asdf",
        }
    }

    /// How to update a mark that this manager installed, when `mark update`
    /// cannot run the manager itself.
    pub(crate) fn update_instructions(self) -> &'static str {
        match self {
            Self::Homebrew => "Update it with:\n  brew upgrade mark-cli",
            Self::Mise => "Update it with:\n  mise upgrade github:phongndo/mark",
            Self::Nix => {
                "Update it with:\n  nix profile upgrade mark\n\nIf a NixOS, nix-darwin, or Home Manager configuration installs it, update its\nmark flake input and rebuild."
            }
            // mark-cli is not on crates.io, so Cargo installs come from Git.
            Self::Cargo => {
                "Update it by rerunning cargo install, for example:\n  cargo install --locked --git https://github.com/phongndo/mark mark-cli"
            }
            Self::Asdf => "Update it with the asdf plugin that installed it.",
        }
    }
}

pub(crate) fn managed_update_refusal(
    install_dir: &Path,
    binary: &OsStr,
    manager: ManagedUpdateInstall,
) -> MarkError {
    usage_error(format!(
        "mark update can't update this mark.\n\nThis mark looks {}-managed:\n  {}\n\n{}",
        manager.name(),
        install_dir.join(binary).display(),
        manager.update_instructions(),
    ))
}

pub(crate) fn update_target_is_current_executable(install_dir: &Path, binary: &OsStr) -> bool {
    let Ok(current_executable) = env::current_exe().and_then(fs::canonicalize) else {
        return false;
    };
    let Ok(update_target) = fs::canonicalize(install_dir.join(binary)) else {
        return false;
    };

    update_target == current_executable
}

pub(crate) fn managed_update_install(
    install_dir: &Path,
    binary: &OsStr,
    mise_data_dir: Option<&Path>,
) -> Option<ManagedUpdateInstall> {
    let target = install_dir.join(binary);
    let classify = |path: &Path| classify_managed_update_path(path, mise_data_dir);

    classify(&target)
        .or_else(|| {
            fs::read_link(&target).ok().and_then(|link| {
                let path = if link.is_absolute() {
                    link
                } else {
                    install_dir.join(link)
                };
                classify(&path)
            })
        })
        .or_else(|| {
            fs::canonicalize(&target)
                .ok()
                .and_then(|path| classify(&path))
        })
        .or_else(|| classify(install_dir))
}

pub(crate) fn classify_managed_update_path(
    path: &Path,
    mise_data_dir: Option<&Path>,
) -> Option<ManagedUpdateInstall> {
    let in_mise_data_dir = mise_data_dir.is_some_and(|dir| {
        path.starts_with(dir.join("shims")) || path.starts_with(dir.join("installs"))
    });
    let path = path.to_string_lossy().replace('\\', "/");

    if path.starts_with("/opt/homebrew/")
        || path.starts_with("/home/linuxbrew/.linuxbrew/")
        || path.contains("/.linuxbrew/")
        || path.contains("/Cellar/")
    {
        return Some(ManagedUpdateInstall::Homebrew);
    }

    if path_has_dir(&path, "/.cargo/bin") {
        return Some(ManagedUpdateInstall::Cargo);
    }

    if in_mise_data_dir
        || path_has_dir(&path, "/mise/shims")
        || path_has_dir(&path, "/mise/installs")
        || path_has_dir(&path, "/.mise/shims")
        || path_has_dir(&path, "/.mise/installs")
    {
        return Some(ManagedUpdateInstall::Mise);
    }

    if path.starts_with("/nix/store/")
        || path_has_dir(&path, "/.nix-profile/bin")
        || path_has_dir(&path, "/.local/state/nix/profile/bin")
        || path.starts_with("/run/current-system/sw/bin")
    {
        return Some(ManagedUpdateInstall::Nix);
    }

    if path_has_dir(&path, "/.asdf/shims") || path_has_dir(&path, "/.asdf/installs") {
        return Some(ManagedUpdateInstall::Asdf);
    }

    None
}

pub(crate) fn path_has_dir(path: &str, dir: &str) -> bool {
    path.ends_with(dir) || path.contains(&format!("{dir}/"))
}

pub(crate) fn default_update_install_dir(argv0: &OsStr) -> MarkResult<PathBuf> {
    let argv0_path = Path::new(argv0);
    if argv0_path.components().count() > 1 {
        return invocation_parent_dir(argv0_path);
    }

    let binary = update_binary_name(argv0)?;
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            if dir.join(Path::new(&binary)).is_file() {
                return absolute_path(dir);
            }
        }
    }

    current_exe_parent_dir()
}

pub(crate) fn invocation_parent_dir(path: &Path) -> MarkResult<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| usage_error("could not determine current executable directory"))?;
    absolute_path(parent.to_path_buf())
}

pub(crate) fn current_exe_parent_dir() -> MarkResult<PathBuf> {
    let executable = env::current_exe()?;
    let parent = executable
        .parent()
        .ok_or_else(|| usage_error("could not determine current executable directory"))?;
    absolute_path(parent.to_path_buf())
}

pub(crate) fn absolute_path(path: PathBuf) -> MarkResult<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_detects_package_manager_install_dirs() {
        let managed = |dir: &str| managed_update_install(Path::new(dir), OsStr::new("mark"), None);

        assert_eq!(
            managed("/opt/homebrew/bin"),
            Some(ManagedUpdateInstall::Homebrew)
        );
        assert_eq!(
            managed("/Users/me/.cargo/bin"),
            Some(ManagedUpdateInstall::Cargo)
        );
        assert_eq!(
            managed("/Users/me/.local/share/mise/shims"),
            Some(ManagedUpdateInstall::Mise)
        );
        // A custom XDG_DATA_HOME still puts mise's directories under mise/.
        assert_eq!(
            managed("/home/me/.data/mise/installs/github-phongndo-mark/1.2.3"),
            Some(ManagedUpdateInstall::Mise)
        );
        assert_eq!(
            managed("/nix/store/abc-mark/bin"),
            Some(ManagedUpdateInstall::Nix)
        );
        assert_eq!(
            classify_managed_update_path(Path::new("/usr/local/bin"), None),
            None
        );

        let mise_data_dir = Some(Path::new("/srv/tools"));
        for dir in [
            "/srv/tools/shims",
            "/srv/tools/installs/github-phongndo-mark/1.2.3",
        ] {
            assert_eq!(
                classify_managed_update_path(Path::new(dir), mise_data_dir),
                Some(ManagedUpdateInstall::Mise),
                "{dir}"
            );
            assert_eq!(classify_managed_update_path(Path::new(dir), None), None);
        }
    }

    #[test]
    fn update_refuses_managed_installs_it_cannot_run() {
        for (install_dir, manager, update_command) in [
            ("/opt/homebrew/bin", "Homebrew", "brew upgrade mark-cli"),
            (
                "/Users/me/.local/share/mise/shims",
                "mise",
                "mise upgrade github:phongndo/mark",
            ),
            (
                "/Users/me/.nix-profile/bin",
                "Nix",
                "nix profile upgrade mark",
            ),
            (
                "/Users/me/.cargo/bin",
                "Cargo",
                "cargo install --locked --git https://github.com/phongndo/mark mark-cli",
            ),
            ("/Users/me/.asdf/shims", "asdf", "asdf plugin"),
        ] {
            let target_manager =
                managed_update_install(Path::new(install_dir), OsStr::new("mark"), None).unwrap();
            // A source build that a package manager directory merely points at.
            let executable = Path::new("/home/me/src/mark/target/release/mark");
            assert_eq!(
                classify_update_method(Some(executable), None, Some(target_manager)),
                UpdateMethod::Refuse(target_manager)
            );

            let error =
                managed_update_refusal(Path::new(install_dir), OsStr::new("mark"), target_manager)
                    .to_string();
            assert!(
                error.contains(&format!("{manager}-managed:\n  {install_dir}/mark")),
                "{error}"
            );
            assert!(error.contains(update_command), "{error}");
            // A second, portable install would shadow or be shadowed by the managed one.
            assert!(!error.contains("install.sh"), "{error}");
            assert!(!error.contains("--install-dir"), "{error}");
            assert!(!error.contains("--force-self-update"), "{error}");
        }

        assert_eq!(
            managed_update_install(
                Path::new("mark-unmanaged-test-bin"),
                OsStr::new("mark"),
                None
            ),
            None
        );
    }

    #[test]
    fn update_runs_the_package_manager_that_installed_the_executable() {
        use ManagedUpdateInstall::{Cargo, Homebrew, Mise, Nix};

        let method = |executable: &str, mise_data_dir: Option<&str>, manager| {
            classify_update_method(
                Some(Path::new(executable)),
                mise_data_dir.map(Path::new),
                Some(manager),
            )
        };

        assert_eq!(
            method(
                "/opt/homebrew/Cellar/mark-cli/1.2.3/bin/mark",
                None,
                Homebrew
            ),
            UpdateMethod::Homebrew {
                prefix: PathBuf::from("/opt/homebrew"),
                formula: "mark-cli".to_owned(),
            }
        );
        assert_eq!(
            method(
                "/home/linuxbrew/.linuxbrew/Cellar/mark-cli/1.2.3/bin/mark",
                None,
                Homebrew
            ),
            UpdateMethod::Homebrew {
                prefix: PathBuf::from("/home/linuxbrew/.linuxbrew"),
                formula: "mark-cli".to_owned(),
            }
        );
        assert_eq!(
            method(
                "/home/me/.local/share/mise/installs/github-phongndo-mark/1.2.3/bin/mark",
                None,
                Mise
            ),
            UpdateMethod::Mise
        );
        assert_eq!(
            method(
                "/opt/mise/installs/github-phongndo-mark/1.2.3/bin/mark",
                Some("/opt/mise"),
                Mise
            ),
            UpdateMethod::Mise
        );
        // MISE_DATA_DIR replaces the default location.
        assert_eq!(
            method(
                "/home/me/.local/share/mise/installs/github-phongndo-mark/1.2.3/bin/mark",
                Some("/opt/mise"),
                Mise
            ),
            UpdateMethod::Refuse(Mise)
        );
        assert_eq!(
            method("/nix/store/abc123-mark-1.2.3/bin/mark", None, Nix),
            UpdateMethod::Nix {
                store_dir: PathBuf::from("/nix/store/abc123-mark-1.2.3"),
            }
        );

        // Only the manager that the install directory checks found is run.
        assert_eq!(
            method("/nix/store/abc123-mark-1.2.3/bin/mark", None, Homebrew),
            UpdateMethod::Refuse(Homebrew)
        );
        assert_eq!(
            method("/home/me/.cargo/bin/mark", None, Cargo),
            UpdateMethod::Refuse(Cargo)
        );
        assert_eq!(
            classify_update_method(None, None, Some(Homebrew)),
            UpdateMethod::Refuse(Homebrew)
        );
    }

    #[test]
    fn update_uses_the_installer_when_no_package_manager_owns_the_target() {
        // A source build or install-script install.
        assert_eq!(
            classify_update_method(
                Some(Path::new("/home/me/src/mark/target/release/mark")),
                None,
                None
            ),
            UpdateMethod::Installer
        );
        // `--install-dir ~/.local/bin` from a Homebrew mark updates that directory.
        assert_eq!(
            classify_update_method(
                Some(Path::new("/opt/homebrew/Cellar/mark-cli/1.2.3/bin/mark")),
                None,
                None
            ),
            UpdateMethod::Installer
        );
    }

    #[test]
    fn homebrew_needs_a_versioned_cellar_path() {
        assert_eq!(
            homebrew_cellar_install(Path::new("/usr/local/Cellar/mark-cli/1.2.3/bin/mark")),
            Some((PathBuf::from("/usr/local"), "mark-cli".to_owned()))
        );
        assert_eq!(
            homebrew_cellar_install(Path::new("/usr/local/Cellar/mark-cli/mark")),
            None
        );
        assert_eq!(
            homebrew_cellar_install(Path::new("/opt/homebrew/bin/mark")),
            None
        );
    }

    #[test]
    fn pinned_version_ignores_latest_and_the_leading_v() {
        assert_eq!(pinned_version(None), None);
        assert_eq!(pinned_version(Some("latest")), None);
        assert_eq!(pinned_version(Some("0.14.1")), Some("0.14.1"));
        assert_eq!(pinned_version(Some("v0.14.1")), Some("0.14.1"));
        assert_eq!(pinned_version(Some("nightly")), Some("nightly"));
    }

    #[test]
    fn nix_profile_element_finds_the_element_with_the_store_path() {
        // `nix profile list --profile <throwaway> --json` from Nix 2.35 and Lix
        // 2.94 after `nix profile add` of this checkout and `nixpkgs#hello`. The
        // name comes from the flake reference, not the package.
        let json = r#"{"elements":{"hello":{"active":true,"attrPath":"legacyPackages.x86_64-linux.hello","originalUrl":"flake:nixpkgs","outputs":null,"priority":5,"storePaths":["/nix/store/2y3s86m1yj9xnypd44vivfz3mwybzlki-hello-2.12.3"],"url":"path:/nix/store/hbd89dn4si5jb3imhymh1bapmf7i5jc4-source"},"mark-wt-update":{"active":true,"attrPath":"packages.x86_64-linux.default","originalUrl":"git+file:///home/dp/code/mark-wt-update","outputs":null,"priority":5,"storePaths":["/nix/store/v86mx3l3hgksfmcwbr9i23b2cp88whv2-mark-0.14.2"],"url":"git+file:///home/dp/code/mark-wt-update"}},"version":3}"#;
        let store_dir = Path::new("/nix/store/v86mx3l3hgksfmcwbr9i23b2cp88whv2-mark-0.14.2");

        assert_eq!(
            nix_profile_element(json, store_dir),
            Some("mark-wt-update".to_owned())
        );
        assert_eq!(
            nix_profile_element(
                json,
                Path::new("/nix/store/00000000000000000000000000000000-mark-0.14.1")
            ),
            None
        );
        assert_eq!(nix_profile_element("not json", store_dir), None);
        assert_eq!(nix_profile_element(r#"{"version":3}"#, store_dir), None);
        // Older Nix versions list elements in an array.
        assert_eq!(
            nix_profile_element(
                r#"{"version":2,"elements":[{"storePaths":["/nix/store/v86mx3l3hgksfmcwbr9i23b2cp88whv2-mark-0.14.2"]}]}"#,
                store_dir
            ),
            None
        );
    }

    #[test]
    fn current_version_only_describes_the_running_update_target() {
        let current_executable = env::current_exe().unwrap();
        let install_dir = current_executable.parent().unwrap();
        let binary = current_executable.file_name().unwrap();

        assert!(update_target_is_current_executable(install_dir, binary));
        assert!(!update_target_is_current_executable(
            install_dir,
            OsStr::new("missing-mark-test-binary")
        ));
    }
}

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use mark_core::MarkResult;

use crate::{
    CliResult,
    args::{INSTALL_SCRIPT, RELEASE_REPO, UpdateArgs},
    version::CLI_VERSION,
};

pub(crate) fn update(args: UpdateArgs) -> CliResult<()> {
    let argv0 = env::args_os().next().ok_or_else(|| {
        mark_core::MarkError::Usage("could not determine current executable".to_owned())
    })?;
    let binary = update_binary_name(&argv0)?;
    let install_dir = match args.install_dir {
        Some(path) => absolute_path(path)?,
        None => default_update_install_dir(&argv0)?,
    };
    check_update_install_dir(&install_dir, &binary)?;
    let updates_current_executable = update_target_is_current_executable(&install_dir, &binary);
    let version = args.version.unwrap_or_else(|| "latest".to_owned());
    let repo = update_repo(env::var_os("MARK_REPO"));

    let mut command = ProcessCommand::new("sh");
    command
        .arg("-s")
        .env("MARK_REPO", repo)
        .env("MARK_INSTALL_DIR", &install_dir)
        .env("MARK_VERSION", version)
        .env_remove("MARK_CURRENT_VERSION")
        .env("MARK_BINARY", &binary)
        .env("MARK_INSTALL_ACTION", "update")
        .stdin(Stdio::piped());
    if updates_current_executable {
        command.env("MARK_CURRENT_VERSION", CLI_VERSION);
    }
    let mut child = command.spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| mark_core::MarkError::Usage("could not open installer stdin".to_owned()))?;
    stdin.write_all(INSTALL_SCRIPT.as_bytes())?;
    drop(stdin);

    let status = child.wait()?;
    if !status.success() {
        return Err(mark_core::MarkError::Usage(format!(
            "update failed with status {}",
            status
                .code()
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        ))
        .into());
    }

    Ok(())
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
        .ok_or_else(|| {
            mark_core::MarkError::Usage("could not determine current executable name".to_owned())
        })
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

    /// How to update a mark that this manager installed. Keep these in sync
    /// with the install methods in the README.
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

pub(crate) fn check_update_install_dir(install_dir: &Path, binary: &OsStr) -> MarkResult<()> {
    let Some(manager) = managed_update_install(install_dir, binary) else {
        return Ok(());
    };

    Err(mark_core::MarkError::Usage(format!(
        "mark update only updates mark installed by the portable installer.\n\nThis mark looks {}-managed:\n  {}\n\n{}",
        manager.name(),
        install_dir.join(binary).display(),
        manager.update_instructions(),
    )))
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
) -> Option<ManagedUpdateInstall> {
    let target = install_dir.join(binary);

    classify_managed_update_path(&target)
        .or_else(|| {
            fs::read_link(&target).ok().and_then(|link| {
                let path = if link.is_absolute() {
                    link
                } else {
                    install_dir.join(link)
                };
                classify_managed_update_path(&path)
            })
        })
        .or_else(|| {
            fs::canonicalize(&target)
                .ok()
                .and_then(|path| classify_managed_update_path(&path))
        })
        .or_else(|| classify_managed_update_path(install_dir))
}

pub(crate) fn classify_managed_update_path(path: &Path) -> Option<ManagedUpdateInstall> {
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

    if path_has_dir(&path, "/.local/share/mise/shims")
        || path_has_dir(&path, "/.local/share/mise/installs")
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
    let parent = path.parent().ok_or_else(|| {
        mark_core::MarkError::Usage("could not determine current executable directory".to_owned())
    })?;
    absolute_path(parent.to_path_buf())
}

pub(crate) fn current_exe_parent_dir() -> MarkResult<PathBuf> {
    let executable = env::current_exe()?;
    let parent = executable.parent().ok_or_else(|| {
        mark_core::MarkError::Usage("could not determine current executable directory".to_owned())
    })?;
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
        assert_eq!(
            managed_update_install(Path::new("/opt/homebrew/bin"), OsStr::new("mark")),
            Some(ManagedUpdateInstall::Homebrew)
        );
        assert_eq!(
            managed_update_install(Path::new("/Users/me/.cargo/bin"), OsStr::new("mark")),
            Some(ManagedUpdateInstall::Cargo)
        );
        assert_eq!(
            managed_update_install(
                Path::new("/Users/me/.local/share/mise/shims"),
                OsStr::new("mark")
            ),
            Some(ManagedUpdateInstall::Mise)
        );
        assert_eq!(
            managed_update_install(Path::new("/nix/store/abc-mark/bin"), OsStr::new("mark")),
            Some(ManagedUpdateInstall::Nix)
        );
        assert_eq!(
            classify_managed_update_path(Path::new("/usr/local/bin")),
            None
        );
    }

    #[test]
    fn update_rejects_managed_install_dirs() {
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
            let error = check_update_install_dir(Path::new(install_dir), OsStr::new("mark"))
                .unwrap_err()
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

        assert!(
            check_update_install_dir(Path::new("mark-unmanaged-test-bin"), OsStr::new("mark"))
                .is_ok()
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

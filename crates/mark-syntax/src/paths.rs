use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    COLORSCHEME_DIR, CONFIG_DIR, CONFIG_FILE, LEGACY_SETTINGS_FILE, SETTINGS_FILE, SyntaxSettings,
    config_home, parse_settings,
};
use mark_core::{MarkError, MarkResult};

pub fn config_path() -> MarkResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(CONFIG_FILE))
}

pub fn settings_path() -> MarkResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(SETTINGS_FILE))
}

pub fn settings_read_path() -> MarkResult<PathBuf> {
    let path = settings_path()?;
    let legacy_path = legacy_settings_path()?;
    Ok(settings_read_path_from_paths(&path, &legacy_path).to_path_buf())
}

pub fn settings_write_path() -> MarkResult<PathBuf> {
    Ok(settings_write_path_from_paths(
        settings_path()?,
        legacy_settings_path()?,
    ))
}

pub(crate) fn legacy_settings_path() -> MarkResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(LEGACY_SETTINGS_FILE))
}

pub(crate) fn settings_write_path_from_paths(
    settings_path: PathBuf,
    legacy_settings_path: PathBuf,
) -> PathBuf {
    if !settings_path.exists() && legacy_settings_path.exists() {
        legacy_settings_path
    } else {
        settings_path
    }
}

pub fn colorscheme_dir() -> MarkResult<PathBuf> {
    config_home().map(|path| path.join(CONFIG_DIR).join(COLORSCHEME_DIR))
}

pub fn load_settings() -> MarkResult<SyntaxSettings> {
    let path = settings_read_path()?;
    load_settings_from_read_path(&path)
}

#[cfg(test)]
pub(crate) fn load_settings_from_path(path: &Path) -> MarkResult<SyntaxSettings> {
    let legacy_path = path.with_file_name(LEGACY_SETTINGS_FILE);
    load_settings_from_paths(path, &legacy_path)
}

#[cfg(test)]
pub(crate) fn load_settings_from_paths(
    path: &Path,
    legacy_path: &Path,
) -> MarkResult<SyntaxSettings> {
    let path = settings_read_path_from_paths(path, legacy_path);
    load_settings_from_read_path(path)
}

fn load_settings_from_read_path(path: &Path) -> MarkResult<SyntaxSettings> {
    if !path.exists() {
        return Ok(SyntaxSettings::default());
    }

    let contents = fs::read_to_string(path)?;
    parse_settings_from_path(path, &contents)
}

fn settings_read_path_from_paths<'a>(path: &'a Path, legacy_path: &'a Path) -> &'a Path {
    if path.exists() || !legacy_path.exists() {
        path
    } else {
        legacy_path
    }
}

fn parse_settings_from_path(path: &Path, contents: &str) -> MarkResult<SyntaxSettings> {
    parse_settings(contents)
        .map_err(|error| MarkError::Usage(format!("failed to parse {}: {error}", path.display())))
}
